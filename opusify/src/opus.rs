use crate::{decoded_chunk::DecodedChunk, ogg, OUT_SAMPLE_RATE};
use anyhow::bail;
use opusic_sys::*;
use std::sync::mpsc::{self};

// it's okay to use a constant here because it has only one stream
const SERIAL: u32 = 12345;

pub fn encode_to_ogg_opus(
    in_rx: mpsc::Receiver<DecodedChunk>,
    err_tx: mpsc::Sender<crate::Error>,
) -> mpsc::Receiver<bytes::Bytes> {
    // Maybe you want to get this docs https://opus-codec.org/docs/opus_api-1.5/group__opus__encoder.html

    let (out_tx, out_rx) = mpsc::sync_channel(128);

    std::thread::spawn(move || {
        let mut writer = ogg::PacketWriter::new(out_tx);

        const MAX_FRAME_SIZE: usize = 2880;
        const MIN_FRAME_SIZE: usize = 120;

        let mut rest = Vec::with_capacity(MAX_FRAME_SIZE);

        let mut encoder: Option<OpusEncoderWrapper> = None;
        let mut lookahead: opus_int32 = 0;
        let mut sample_acc = 0;

        while let Ok(chunk) = in_rx.recv() {
            let channels = chunk.channels;

            if encoder.is_none() {
                unsafe {
                    let mut error = 0;
                    let encoder_ptr = opus_encoder_create(
                        OUT_SAMPLE_RATE as _,
                        channels as _,
                        OPUS_APPLICATION_AUDIO,
                        &mut error,
                    );
                    if error != 0 {
                        let _ = err_tx.send(crate::Error::OpusEncode {
                            reason: std::ffi::CStr::from_ptr(opus_strerror(error))
                                .to_str()
                                .unwrap(),
                        });
                        return;
                    }

                    encoder = Some(OpusEncoderWrapper { ptr: encoder_ptr });

                    let encoder = encoder.as_ref().unwrap().ptr;

                    let error =
                        opus_encoder_ctl(encoder, OPUS_GET_LOOKAHEAD_REQUEST, &mut lookahead);
                    if error != 0 {
                        let _ = err_tx.send(crate::Error::OpusEncode {
                            reason: std::ffi::CStr::from_ptr(opus_strerror(error))
                                .to_str()
                                .unwrap(),
                        });
                        return;
                    }

                    {
                        // https://wiki.xiph.org/OggOpus#ID_Header
                        //  0                   1                   2                   3
                        //  0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
                        // +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
                        // |       'O'     |      'p'      |     'u'       |     's'       |
                        // +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
                        // |       'H'     |       'e'     |     'a'       |     'd'       |
                        // +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
                        // |  version = 1  | channel count |           pre-skip            |
                        // +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
                        // |                original input sample rate in Hz               |
                        // +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
                        // |    output gain Q7.8 in dB     |  channel map  |               |
                        // +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+               :
                        // |                                                               |
                        // :          optional channel mapping table...                    :
                        // |                                                               |
                        // +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
                        let mut head = Vec::with_capacity(19);
                        head.extend("OpusHead".bytes());
                        head.push(1);
                        head.push(channels as u8);
                        head.extend((lookahead as u16).to_le_bytes());
                        head.extend(48000u32.to_le_bytes());
                        head.extend(0u16.to_le_bytes()); // Output gain
                        head.push(0);

                        assert_eq!(head.len(), 19);

                        if writer
                            .write_packet(head, SERIAL, ogg::PacketWriteEndInfo::EndPage, 0)
                            .is_err()
                        {
                            return;
                        }
                    }

                    {
                        let mut opus_tags: Vec<u8> = Vec::with_capacity(60);
                        opus_tags.extend(b"OpusTags");

                        let vendor_str = "namui-ogg-opus";
                        opus_tags.extend(&(vendor_str.len() as u32).to_le_bytes());
                        opus_tags.extend(vendor_str.bytes());

                        opus_tags.extend(&[0u8; 4]); // No user comments

                        if writer
                            .write_packet(opus_tags, SERIAL, ogg::PacketWriteEndInfo::EndPage, 0)
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }

            let encoder = encoder.as_ref().unwrap().ptr;

            let mut pcm = chunk.pcm.as_slice();

            if !rest.is_empty() {
                let amount_to_rest = (MAX_FRAME_SIZE - rest.len()).min(pcm.len());
                rest.extend_from_slice(pcm[..amount_to_rest].as_ref());
                pcm = &pcm[amount_to_rest..];

                assert!(rest.len() <= MAX_FRAME_SIZE);

                if rest.len() != MAX_FRAME_SIZE {
                    continue;
                }

                if encode_and_send(
                    encoder,
                    &mut writer,
                    &err_tx,
                    lookahead,
                    &mut sample_acc,
                    std::mem::take(&mut rest).as_slice(),
                    MIN_FRAME_SIZE,
                )
                .is_err()
                {
                    return;
                };
            }

            let send_pcm_len = MAX_FRAME_SIZE * channels;

            while pcm.len() >= send_pcm_len {
                if encode_and_send(
                    encoder,
                    &mut writer,
                    &err_tx,
                    lookahead,
                    &mut sample_acc,
                    &pcm[..send_pcm_len],
                    MIN_FRAME_SIZE,
                )
                .is_err()
                {
                    return;
                }
                pcm = &pcm[send_pcm_len..];
            }

            rest.extend_from_slice(pcm);
        }

        assert!(rest.len() <= MAX_FRAME_SIZE);
        rest.resize(MAX_FRAME_SIZE, 0);

        if let Some(encoder) = encoder {
            let encoder = encoder.ptr;
            let _ = encode_and_send(
                encoder,
                &mut writer,
                &err_tx,
                lookahead,
                &mut sample_acc,
                rest.as_slice(),
                MIN_FRAME_SIZE,
            );
        }
    });

    out_rx
}

struct OpusEncoderWrapper {
    ptr: *mut OpusEncoder,
}

impl Drop for OpusEncoderWrapper {
    fn drop(&mut self) {
        unsafe {
            opus_encoder_destroy(self.ptr);
        }
    }
}
fn encode_and_send(
    encoder: *mut OpusEncoder,
    writer: &mut ogg::PacketWriter,
    err_tx: &mpsc::Sender<crate::Error>,
    lookahead: opus_int32,
    sample_acc: &mut usize,
    pcm: &[i16],
    frame_size: usize,
) -> anyhow::Result<()> {
    let mut output_buffer: Vec<u8> = vec![0; 8192];

    let output_len = unsafe {
        let output_len = opus_encode(
            encoder,
            pcm.as_ptr(),
            frame_size as _,
            output_buffer.as_mut_ptr(),
            output_buffer.len() as _,
        );
        if output_len < 0 {
            let error = output_len;
            err_tx.send(crate::Error::OpusEncode {
                reason: std::ffi::CStr::from_ptr(opus_strerror(error))
                    .to_str()
                    .unwrap(),
            })?;
            bail!("");
        }
        output_len
    } as usize;

    output_buffer.truncate(output_len);

    *sample_acc += frame_size;

    // https://wiki.xiph.org/OggOpus#Granule_Position
    let granule_position = lookahead as usize + *sample_acc;

    writer.write_packet(
        output_buffer,
        SERIAL,
        ogg::PacketWriteEndInfo::NormalPacket,
        granule_position as u64,
    )?;

    Ok(())
}
