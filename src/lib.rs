// Copyright 2019 J. S. Mueller-Roemer

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at

//    http://www.apache.org/licenses/LICENSE-2.0

// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use lazy_static::lazy_static;

use std::f32::consts::PI;
use std::sync::{Arc, RwLock};

use vst::buffer::AudioBuffer;
use vst::plugin::{Category, Info, Plugin, PluginParameters};
use vst::plugin_main;

struct FaderEffect {
	parameters: Arc<FaderEffectParameters>,
	current_volume: f32,
	current_pan: f32,
}

impl Default for FaderEffect {
	fn default() -> FaderEffect {
		FaderEffect {
			parameters: Arc::new(FaderEffectParameters::default()),
			current_volume: -1.0, // these should really be reset to -1.0 whenever processing is interrupted or transport is moved etc.
			current_pan: -1.0,
		}
	}
}

impl FaderEffect {
	fn process_internal<F: num_traits::Float + std::convert::From<f32>>(
		&mut self,
		buffer: &mut AudioBuffer<F>,
	) {
		let parameters = self.parameters.storage.read().unwrap();
		let target_volume = parameters.volume;
		let target_pan = parameters.pan;
		let pan_taper = &PAN_LUT[std::cmp::min((parameters.pan_taper * 2.0) as usize, 1)];
		let pan_law = &pan_taper[std::cmp::min((parameters.pan_law * 3.0) as usize, 2)];
		let volume_lut = &*VOLUME_LUT;

		let mut volume = if self.current_volume < 0.0 {
			target_volume
		} else {
			self.current_volume
		};
		self.current_volume = target_volume;

		let mut pan = if self.current_pan < 0.0 {
			target_pan
		} else {
			self.current_pan
		};
		self.current_pan = target_pan;

		let num_samples = buffer.samples();
		let (inputs, outputs) = buffer.split();
		let num_inputs = inputs.len();
		let num_outputs = outputs.len();
		let num_channels = std::cmp::min(num_inputs, num_outputs);
		let channel_pairs = num_channels / 2;

		let volume_delta = (target_volume - volume) / num_samples as f32;
		let pan_delta = (target_pan - pan) / num_samples as f32;

		for sample_idx in 0..num_samples {
			volume = (volume + volume_delta).max(0.0).min(1.0);
			pan = (pan + pan_delta).max(0.0).min(1.0);

			let volume_gain: F = lookup_interpolated(volume_lut, volume).into();
			let gain_left = volume_gain * lookup_interpolated(pan_law, pan).into();
			let gain_right = volume_gain * lookup_interpolated(pan_law, 1.0 - pan).into();

			for channel_pair in 0..channel_pairs {
				let left_index = 2 * channel_pair;
				let right_index = left_index + 1;

				let input_left = non_finite_to_zero(inputs.get(left_index)[sample_idx]);
				let input_right = non_finite_to_zero(inputs.get(right_index)[sample_idx]);
				let output_left = &mut outputs.get_mut(left_index)[sample_idx];
				let output_right = &mut outputs.get_mut(right_index)[sample_idx];

				*output_left = gain_left * input_left;
				*output_right = gain_right * input_right;
			}

			if 2 * channel_pairs != num_channels {
				outputs.get_mut(num_channels - 1)[sample_idx] =
					volume_gain * non_finite_to_zero(inputs.get(num_channels - 1)[sample_idx]);
			}

			for index in num_channels..num_outputs {
				outputs.get_mut(index)[sample_idx] = F::zero();
			}
		}
	}
}

impl Plugin for FaderEffect {
	fn get_info(&self) -> Info {
		Info {
			name: "jsFader (Rust Edition)".to_string(),
			vendor: "jsPlugs".to_string(),
			unique_id: 0x6a73_4661 ^ 0x0000_ffff,
			version: 1,
			inputs: 2,
			outputs: 2,
			parameters: 4,
			category: Category::Effect,
			f64_precision: true,
			silent_when_stopped: true,
			..Default::default()
		}
	}

	fn process(&mut self, buffer: &mut AudioBuffer<f32>) {
		self.process_internal(buffer);
	}

	fn process_f64(&mut self, buffer: &mut AudioBuffer<f64>) {
		self.process_internal(buffer);
	}

	fn get_parameter_object(&mut self) -> Arc<dyn PluginParameters> {
		Arc::clone(&self.parameters) as Arc<dyn PluginParameters>
	}
}

plugin_main!(FaderEffect);

#[derive(Clone, Copy)]
struct FaderEffectParameterStorage {
	volume: f32,
	pan: f32,
	pan_taper: f32,
	pan_law: f32,
}

struct FaderEffectParameters {
	storage: RwLock<FaderEffectParameterStorage>,
}

impl Default for FaderEffectParameters {
	fn default() -> FaderEffectParameters {
		FaderEffectParameters {
			storage: RwLock::new(FaderEffectParameterStorage {
				volume: 0.75,
				pan: 0.5,
				pan_taper: 0.0,
				pan_law: 0.5,
			}),
		}
	}
}

impl PluginParameters for FaderEffectParameters {
	fn get_parameter(&self, index: i32) -> f32 {
		let storage = self.storage.read().unwrap();
		match index {
			0 => storage.volume,
			1 => storage.pan,
			2 => storage.pan_taper,
			3 => storage.pan_law,
			_ => {
				// release lock before panicking!
				drop(storage);
				panic!("invalid parameter index!")
			}
		}
	}

	fn set_parameter(&self, index: i32, value: f32) {
		let mut storage = self.storage.write().unwrap();
		match index {
			0 => storage.volume = value,
			1 => storage.pan = value,
			2 => storage.pan_taper = ((value * 2.0) as i32 as f32).max(0.0).min(1.0),
			3 => storage.pan_law = ((value * 3.0) as i32 as f32 / 2.0).max(0.0).min(1.0),
			_ => {
				// release lock before panicking!
				drop(storage);
				panic!("invalid parameter index!")
			}
		}
	}

	fn get_parameter_text(&self, index: i32) -> String {
		match index {
			0 => {
				let volume = self.storage.read().unwrap().volume;
				let gain = lookup_interpolated(&*VOLUME_LUT, volume);
				if gain < 1e-5 {
					"-inf dB".to_string()
				} else {
					format!("{:+.1} dB", 20.0 * gain.log10())
				}
			}
			1 => {
				let pan = (200.0 * (self.storage.read().unwrap().pan - 0.5)).round() as i32;
				format!(
					"{} {}",
					pan.abs(),
					if pan < 0 {
						"L"
					} else if pan > 0 {
						"R"
					} else {
						"C"
					}
				)
			}
			2 => {
				let index = (self.storage.read().unwrap().pan_taper * 2.0) as i32;
				match index {
					0 => "Sine".to_string(),
					_ => "Root".to_string(),
				}
			}
			3 => {
				let index = (self.storage.read().unwrap().pan_law * 3.0) as i32;
				match index {
					0 => "3 dB".to_string(),
					1 => "4.5 dB".to_string(),
					_ => "6 dB".to_string(),
				}
			}
			_ => panic!("invalid parameter index!"),
		}
	}

	fn get_parameter_name(&self, index: i32) -> String {
		match index {
			0 => "Volume",
			1 => "Pan",
			2 => "Pan Taper",
			3 => "Pan Law",
			_ => panic!("invalid parameter index!"),
		}
		.to_string()
	}
}

fn sinusoidal_pan(pan: f32, law: f32) -> f32 {
	(0.5 * PI * (1. - pan)).sin().powf(law / 3.0)
}

fn root_pan(pan: f32, law: f32) -> f32 {
	(1.0 - pan).powf(law / 6.0)
}

const PAN_TAPERS: [fn(f32, f32) -> f32; 2] = {
	let mut tapers: [fn(f32, f32) -> f32; 2] = [sinusoidal_pan; 2];
	tapers[1] = root_pan;
	tapers
};

const PAN_LAWS: [f32; 3] = {
	let mut laws = [3.0f32; 3];
	laws[1] = 4.5;
	laws[2] = 6.0;
	laws
};

lazy_static! {
	static ref VOLUME_LUT: [f32; 10] = {
		let mut lut = [0.0f32; 10];
		lut[0] = 0.0;
		lut[1] = 10.0f32.powf(-2.25);
		lut[2] = 10.0f32.powf(-1.5);
		lut[3] = 10.0f32.powf(-1.0);
		lut[4] = 10.0f32.powf(-0.5);
		lut[5] = 10.0f32.powf(-0.25);
		lut[6] = 1.0;
		lut[7] = 10.0f32.powf(0.25);
		lut[8] = 10.0f32.powf(0.5);
		lut[9] = lut[8];
		lut
	};
	static ref PAN_LUT: [[[f32; 202]; 3]; 2] = {
		let mut lut = [[[0.0f32; 202]; 3]; 2];
		for (taper, taper_lut) in PAN_TAPERS.iter().zip(lut.iter_mut()) {
			for (law, law_lut) in PAN_LAWS.iter().copied().zip(taper_lut.iter_mut()) {
				#[allow(clippy::needless_range_loop)]
				for index in 0..201 {
					let pan_amount = index as f32 / 200.0;
					law_lut[index] = taper(pan_amount, law);
				}
				law_lut[201] = law_lut[200];
			}
		}
		lut
	};
}

fn lookup_interpolated(lut: &[f32], pos: f32) -> f32 {
	let mut t = (lut.len() - 2) as f32 * pos;
	let index = t as usize;
	t -= index as f32;
	(1.0 - t) * lut[index] + t * lut[index + 1]
}

fn non_finite_to_zero<F: num_traits::Float>(value: F) -> F {
	if value.is_finite() {
		value
	} else {
		F::zero()
	}
}
