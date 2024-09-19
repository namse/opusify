mod decoded_chunk;
#[allow(non_camel_case_types)]
#[allow(non_upper_case_globals)]
#[allow(dead_code)]
mod minimp3_bindings;
mod mp3;
mod opus;
mod resample;

use anyhow::bail;
use std::io::Read;

const OUT_SAMPLE_RATE: usize = 48000;

pub fn opusify(path: impl AsRef<std::path::Path>) -> anyhow::Result<Vec<u8>> {
    let (bytes_tx, bytes_rx) = std::sync::mpsc::channel();
    let (err_tx, err_rx) = std::sync::mpsc::channel();

    let out_rx = mp3::decode_mp3(bytes_rx);
    let out_rx = resample::resample(out_rx, err_tx.clone());
    spawn_file_reader(path, bytes_tx, err_tx.clone())?;
    let out_rx = opus::encode_to_ogg_opus(out_rx, err_tx.clone())?;

    let mut output = Vec::new();
    while let Ok(bytes) = out_rx.recv() {
        output.extend_from_slice(&bytes);
    }
    println!("opusify finished, output size: {}", output.len());

    if let Ok(error) = err_rx.try_recv() {
        bail!(error);
    }

    Ok(output)
}

fn spawn_file_reader(
    path: impl AsRef<std::path::Path>,
    bytes_tx: std::sync::mpsc::Sender<bytes::Bytes>,
    err_tx: std::sync::mpsc::Sender<Error>,
) -> anyhow::Result<()> {
    let mut file = std::fs::File::open(path)?;
    std::thread::spawn(move || {
        let mut read_acc = 0;
        let result: anyhow::Result<()> = (|| {
            loop {
                let mut buf = vec![0u8; 32 * 1024];
                let read = file.read(&mut buf)?;
                read_acc += read;
                if read == 0 {
                    break;
                }
                buf.truncate(read);
                let bytes = bytes::Bytes::from(buf);
                bytes_tx.send(bytes)?;
            }

            println!("reading thread finished, read {} bytes", read_acc);

            Ok(())
        })();
        if let Err(error) = result {
            let _ = err_tx.send(crate::Error::ByteRecv { error });
        }
    });
    Ok(())
}

#[derive(Debug)]
pub enum Error {
    ByteRecv {
        error: anyhow::Error,
    },
    Resample {
        error: rubato::ResamplerConstructionError,
    },
    OpusEncode {
        reason: &'static str,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for Error {}
