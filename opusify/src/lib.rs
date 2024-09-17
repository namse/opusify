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
mod resample;

use anyhow::*;

pub async fn opusify<Input>(input: Input) -> Result<()> {
    // mp3::mp3();
    todo!()
}
