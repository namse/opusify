use crate::{decoded_chunk::DecodedChunk, minimp3_bindings::*};
use std::sync::mpsc;

pub fn decode_mp3(in_rx: mpsc::Receiver<bytes::Bytes>) -> mpsc::Receiver<DecodedChunk> {
    let (out_tx, out_rx) = mpsc::sync_channel(128);

    std::thread::spawn(move || {
        let mut mp3_decoder = unsafe {
            let mut mp3_decoder: mp3dec_t = std::mem::zeroed();
            mp3dec_init(&mut mp3_decoder);
            mp3_decoder
        };

        let mut info: mp3dec_frame_info_t = unsafe { std::mem::zeroed() };

        let mut mp3_input_buffer = Vec::with_capacity(32 * 1024);
        let mut head = 0;
        let mut tail = 0;

        let mut buffer = vec![0i16; MINIMP3_MAX_SAMPLES_PER_FRAME as usize];

        while let Ok(chunk) = in_rx.recv() {
            mp3_input_buffer.extend_from_slice(&chunk);
            tail += chunk.len();

            // Note: We recommend having as many as 10 consecutive MP3 frames (~16KB) in the input buffer at a time.
            // written in https://github.com/lieff/minimp3
            while tail - head > 16 * 1024 {
                let samples = unsafe {
                    mp3dec_decode_frame(
                        &mut mp3_decoder,
                        mp3_input_buffer.as_ptr().add(head),
                        (tail - head) as _,
                        buffer.as_mut_ptr(),
                        &mut info,
                    )
                };

                if samples == 0 && info.frame_bytes == 0 {
                    // not enough data
                    break;
                }

                if samples != 0 {
                    let pcm = buffer[..samples as usize].to_vec();
                    if out_tx
                        .send(DecodedChunk {
                            pcm,
                            channels: info.channels as _,
                            sample_rate: info.hz as _,
                        })
                        .is_err()
                    {
                        return;
                    }
                }

                head += info.frame_bytes as usize;
            }

            if head > 0 {
                mp3_input_buffer.copy_within(head..tail, 0);
                tail -= head;
                head = 0;
                mp3_input_buffer.truncate(tail);
            }
        }
    });

    out_rx
}
