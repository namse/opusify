use crate::{decoded_chunk::DecodedChunk, OUT_SAMPLE_RATE};
use rubato::*;
use std::sync::mpsc;

pub fn resample(
    in_rx: mpsc::Receiver<DecodedChunk>,
    err_tx: mpsc::Sender<crate::Error>,
) -> mpsc::Receiver<DecodedChunk> {
    let (out_tx, out_rx) = mpsc::channel();

    std::thread::spawn({
        move || {
            let now = std::time::Instant::now();
            let mut samples_sum = 0;
            let result: anyhow::Result<()> = (|| {
                let mut chunk = in_rx.recv()?;

                let sample_rate = chunk.sample_rate;
                let channels = chunk.channels;

                let mut resampler = FftFixedIn::<f32>::new(
                    sample_rate as _,
                    OUT_SAMPLE_RATE as _,
                    chunk.pcm.len() / channels,
                    8,
                    channels,
                )?;

                let mut wave_in = vec![0f32; chunk.pcm.len()];

                loop {
                    samples_sum += chunk.pcm.len() / channels;
                    if channels == 1 {
                        chunk.pcm.iter().enumerate().for_each(|(index, &sample)| {
                            wave_in[index] = sample as f32 / i16::MAX as f32;
                        });

                        let mut resampled = Resampler::process(&mut resampler, &[&wave_in], None)?;

                        out_tx.send(DecodedChunk {
                            sample_rate: OUT_SAMPLE_RATE,
                            channels,
                            pcm: resampled
                                .remove(0)
                                .into_iter()
                                .map(|sample| (sample * i16::MAX as f32) as i16)
                                .collect(),
                        })?;
                    } else {
                        assert_eq!(channels, 2);
                        let (left, right) = wave_in.split_at_mut(chunk.pcm.len() / 2);
                        chunk
                            .pcm
                            .chunks_exact(2)
                            .enumerate()
                            .for_each(|(i, chunk)| {
                                left[i] = chunk[0] as f32 / i16::MAX as f32;
                                right[i] = chunk[1] as f32 / i16::MAX as f32;
                            });

                        let resampled = Resampler::process(&mut resampler, &[&left, &right], None)?;

                        out_tx.send(DecodedChunk {
                            sample_rate: OUT_SAMPLE_RATE,
                            channels,
                            // interleaved
                            pcm: resampled[0]
                                .iter()
                                .zip(resampled[1].iter())
                                .flat_map(|(left, right)| {
                                    [
                                        (left * i16::MAX as f32) as i16,
                                        (right * i16::MAX as f32) as i16,
                                    ]
                                })
                                .collect(),
                        })?;
                    }

                    chunk = in_rx.recv()?;
                }
            })();

            println!(
                "resampler thread finished, processed {} samples, elapsed: {:?}",
                samples_sum,
                now.elapsed()
            );

            if let Err(err) = result {
                if let Ok(error) = err.downcast::<ResamplerConstructionError>() {
                    let _ = err_tx.send(crate::Error::Resample { error });
                }
            }
        }
    });

    out_rx
}
