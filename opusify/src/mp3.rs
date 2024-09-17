use crate::{decoded_chunk::DecodedChunk, minimp3_bindings::*};
use futures::{TryStream, TryStreamExt};

pub async fn decode_mp3<Error>(
    stream: impl TryStream<Ok = bytes::Bytes, Error = Error> + std::marker::Unpin,
) -> impl TryStream<Ok = DecodedChunk, Error = Error> {
    let mut mp3_decoder = unsafe {
        let mut mp3_decoder: mp3dec_t = std::mem::zeroed();
        mp3dec_init(&mut mp3_decoder);
        mp3_decoder
    };

    let mut info: mp3dec_frame_info_t = unsafe { std::mem::zeroed() };

    let mut mp3_input_buffer = Vec::with_capacity(32 * 1024);
    let mut head = 0;
    let mut tail = 0;

    let mut pcm = vec![0i16; MINIMP3_MAX_SAMPLES_PER_FRAME as usize];

    stream
        .and_then(move |stream_chunk| {
            mp3_input_buffer.extend_from_slice(&stream_chunk);
            tail += stream_chunk.len();

            let mut outputs = vec![];

            // Note: We recommend having as many as 10 consecutive MP3 frames (~16KB) in the input buffer at a time.
            // written in https://github.com/lieff/minimp3

            while tail - head > 16 * 1024 {
                let samples = unsafe {
                    mp3dec_decode_frame(
                        &mut mp3_decoder,
                        mp3_input_buffer.as_ptr().add(head),
                        (tail - head) as _,
                        pcm.as_mut_ptr(),
                        &mut info,
                    )
                };

                if samples == 0 && info.frame_bytes == 0 {
                    // not enough data
                    break;
                }

                if samples != 0 {
                    outputs.push(pcm[..samples as usize].to_vec());
                }

                head += info.frame_bytes as usize;
            }

            if head > 0 {
                mp3_input_buffer.copy_within(head..tail, 0);
                tail -= head;
                head = 0;
                mp3_input_buffer.truncate(tail);
            }

            async move {
                Ok(DecodedChunk {
                    pcms: outputs,
                    channels: info.channels as _,
                    hz: info.hz as _,
                })
            }
        })
        .try_filter(|chunk| futures::future::ready(!chunk.pcms.is_empty()))
}
