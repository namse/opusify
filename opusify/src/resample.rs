use crate::{decoded_chunk::DecodedChunk, libswresample_bindings::*, OUT_SAMPLE_RATE};
use std::sync::mpsc;

pub fn resample(
    in_rx: mpsc::Receiver<DecodedChunk>,
    err_tx: mpsc::Sender<crate::Error>,
) -> mpsc::Receiver<DecodedChunk> {
    // Maybe you want to get this docs https://www.ffmpeg.org/doxygen/7.0/group__lswr.html

    let (out_tx, out_rx) = mpsc::sync_channel(128);

    std::thread::spawn({
        move || {
            let err = 'outer: {
                let mut ctx = None;
                let mut channels = None;

                while let Ok(chunk) = in_rx.recv() {
                    if ctx.is_none() {
                        channels = Some(chunk.channels);
                        unsafe {
                            let mut ctx_ptr = std::ptr::null_mut();
                            let mut channel_layout: AVChannelLayout = std::mem::zeroed();
                            av_channel_layout_default(&mut channel_layout, chunk.channels as _);

                            let error = swr_alloc_set_opts2(
                                &mut ctx_ptr,
                                &channel_layout,
                                AVSampleFormat_AV_SAMPLE_FMT_S16,
                                OUT_SAMPLE_RATE as _,
                                &channel_layout,
                                AVSampleFormat_AV_SAMPLE_FMT_S16,
                                chunk.sample_rate as _,
                                0,
                                std::ptr::null_mut(),
                            );
                            if error != 0 {
                                break 'outer Err(crate::Error::Resample {
                                    reason: error_to_str(error),
                                });
                            }

                            ctx = Some(SwrContextWrapper { ptr: ctx_ptr });

                            let error = swr_init(ctx_ptr);
                            if error != 0 {
                                break 'outer Err(crate::Error::Resample {
                                    reason: error_to_str(error),
                                });
                            }
                        }
                    }

                    let ctx = ctx.as_ref().unwrap().ptr;
                    let channels = channels.unwrap();

                    unsafe {
                        let maybe_out_samples =
                            swr_get_out_samples(ctx, chunk.pcm.len() as _) as usize;
                        let mut out_buffer = vec![0i16; maybe_out_samples * channels];
                        let out_samples = swr_convert(
                            ctx,
                            out_buffer.as_mut_ptr().cast(),
                            (maybe_out_samples / channels) as _,
                            chunk.pcm.as_ptr().cast(),
                            (chunk.pcm.len() / channels) as _,
                        );
                        if out_samples < 0 {
                            let error = out_samples;
                            break 'outer Err(crate::Error::Resample {
                                reason: error_to_str(error),
                            });
                        }

                        out_buffer.truncate(out_samples as usize * channels);

                        if out_tx
                            .send(DecodedChunk {
                                pcm: out_buffer,
                                channels,
                                sample_rate: OUT_SAMPLE_RATE,
                            })
                            .is_err()
                        {
                            break 'outer Ok(());
                        }
                    }
                }

                if let Some(ctx) = ctx {
                    let ctx = ctx.ptr;
                    let channels = channels.unwrap();
                    unsafe {
                        let maybe_out_samples = swr_get_out_samples(ctx, 0) as usize;
                        let mut out_buffer = vec![0i16; maybe_out_samples * channels];
                        let out_samples = swr_convert(
                            ctx,
                            out_buffer.as_mut_ptr().cast(),
                            maybe_out_samples as _,
                            std::ptr::null(),
                            0,
                        );
                        if out_samples < 0 {
                            let error = out_samples;
                            break 'outer Err(crate::Error::Resample {
                                reason: error_to_str(error),
                            });
                        }

                        out_buffer.truncate(out_samples as usize * channels);

                        if out_tx
                            .send(DecodedChunk {
                                pcm: out_buffer,
                                channels,
                                sample_rate: OUT_SAMPLE_RATE,
                            })
                            .is_err()
                        {
                            break 'outer Ok(());
                        }
                    }
                }

                Ok(())
            };
            if let Err(err) = err {
                err_tx.send(err).unwrap();
            }
        }
    });

    out_rx
}

fn error_to_str(error: i32) -> String {
    const ERRBUF_SIZE: usize = 256;
    let mut errbuf = [0; ERRBUF_SIZE];
    unsafe {
        assert_eq!(av_strerror(error, errbuf.as_mut_ptr(), ERRBUF_SIZE), 0);
        std::ffi::CStr::from_ptr(errbuf.as_ptr())
            .to_str()
            .unwrap()
            .to_string()
    }
}

struct SwrContextWrapper {
    ptr: *mut SwrContext,
}
impl Drop for SwrContextWrapper {
    fn drop(&mut self) {
        unsafe {
            swr_close(self.ptr);
            swr_free(&mut self.ptr);
        }
    }
}
