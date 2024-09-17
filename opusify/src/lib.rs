mod decoded_chunk;
#[allow(non_camel_case_types)]
#[allow(non_upper_case_globals)]
#[allow(dead_code)]
#[allow(clippy::all)]
#[allow(improper_ctypes)]
#[allow(non_snake_case)]
mod libswresample_bindings;
#[allow(non_camel_case_types)]
#[allow(non_upper_case_globals)]
#[allow(dead_code)]
mod minimp3_bindings;
mod mp3;
mod ogg;
mod opus;
mod resample;

use anyhow::*;

const OUT_SAMPLE_RATE: usize = 48000;

pub async fn opusify<Input>(input: Input) -> Result<()> {
    // mp3::decode_mp3();
    // resample::resample(in_rx, err_tx);
    todo!()
}

pub enum Error {
    Resample { reason: String },
    OpusEncode { reason: &'static str },
}
