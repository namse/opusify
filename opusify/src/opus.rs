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

    let (out_tx, out_rx) = mpsc::channel();

    std::thread::spawn(move || {
        let result: anyhow::Result<()> = (|| {
            let mut pcm_count = 0;
            let mut writer = ogg::PacketWriter::new(out_tx);

            const MAX_FRAME_SIZE: usize = 2880;
            // const MIN_FRAME_SIZE: usize = 120;

            let mut rest = Vec::with_capacity(MAX_FRAME_SIZE * 2);

            let mut encoder: Option<OpusEncoderWrapper> = None;
            let mut lookahead: opus_int32 = 0;
            let mut sample_acc = 0;
            let mut channels = 0;

            while let Ok(chunk) = in_rx.recv() {
                pcm_count += chunk.pcm.len();

                channels = chunk.channels;

                if encoder.is_none() {
                    encoder = Some(init_encoder(&mut writer, channels, &mut lookahead)?);
                }

                let encoder = encoder.as_ref().unwrap().ptr;

                let mut pcm = chunk.pcm.as_slice();
                let frame_size = MAX_FRAME_SIZE;
                let send_pcm_len = frame_size * channels;

                if !rest.is_empty() {
                    let number_to_fill_rest = (send_pcm_len - rest.len()).min(pcm.len());
                    rest.extend_from_slice(&pcm[..number_to_fill_rest]);
                    pcm = &pcm[number_to_fill_rest..];

                    assert!(rest.len() <= send_pcm_len);

                    if rest.len() < send_pcm_len {
                        continue;
                    }

                    encode_and_send(
                        encoder,
                        &mut writer,
                        lookahead,
                        &mut sample_acc,
                        rest.as_slice(),
                        frame_size,
                        false,
                    )?;
                    rest.clear();
                }

                while pcm.len() >= send_pcm_len {
                    encode_and_send(
                        encoder,
                        &mut writer,
                        lookahead,
                        &mut sample_acc,
                        &pcm[..send_pcm_len],
                        frame_size,
                        false,
                    )?;
                    pcm = &pcm[send_pcm_len..];
                }

                rest.extend_from_slice(pcm);
            }

            if let Some(encoder) = encoder {
                let encoder = encoder.ptr;

                assert_ne!(channels, 0);
                assert!(rest.len() <= MAX_FRAME_SIZE * channels);
                rest.resize(MAX_FRAME_SIZE * channels, 0);

                encode_and_send(
                    encoder,
                    &mut writer,
                    lookahead,
                    &mut sample_acc,
                    rest.as_slice(),
                    MAX_FRAME_SIZE, // TODO: Use MIN_FRAME_SIZE to reduce end padding
                    true,
                )?;
            }

            println!("opus thread finished, processed {} samples", pcm_count);

            Ok(())
        })();

        if let Err(error) = result {
            if let Ok(error) = error.downcast::<crate::Error>() {
                let _ = err_tx.send(error);
            }
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
    lookahead: opus_int32,
    sample_acc: &mut usize,
    pcm: &[i16],
    frame_size: usize,
    is_end: bool,
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
            bail!(crate::Error::OpusEncode {
                reason: std::ffi::CStr::from_ptr(opus_strerror(error))
                    .to_str()
                    .unwrap(),
            });
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
        if is_end {
            ogg::PacketWriteEndInfo::EndStream
        } else {
            ogg::PacketWriteEndInfo::NormalPacket
        },
        granule_position as u64,
    )?;

    Ok(())
}

fn write_header(
    writer: &mut ogg::PacketWriter,
    channels: usize,
    lookahead: usize,
) -> anyhow::Result<()> {
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

    writer.write_packet(head, SERIAL, ogg::PacketWriteEndInfo::EndPage, 0)?;
    Ok(())
}

fn write_tags(writer: &mut ogg::PacketWriter) -> anyhow::Result<()> {
    let mut opus_tags: Vec<u8> = Vec::with_capacity(60);
    opus_tags.extend(b"OpusTags");

    let vendor_str = "namui-ogg-opus";
    opus_tags.extend(&(vendor_str.len() as u32).to_le_bytes());
    opus_tags.extend(vendor_str.bytes());

    opus_tags.extend(&[0u8; 4]); // No user comments

    writer.write_packet(opus_tags, SERIAL, ogg::PacketWriteEndInfo::EndPage, 0)?;
    Ok(())
}

fn init_encoder(
    writer: &mut ogg::PacketWriter,
    channels: usize,
    lookahead: &mut opus_int32,
) -> anyhow::Result<OpusEncoderWrapper> {
    let encoder = unsafe {
        let mut error = 0;
        let encoder_ptr = opus_encoder_create(
            OUT_SAMPLE_RATE as _,
            channels as _,
            OPUS_APPLICATION_AUDIO,
            &mut error,
        );
        if error != 0 {
            bail!(crate::Error::OpusEncode {
                reason: std::ffi::CStr::from_ptr(opus_strerror(error))
                    .to_str()
                    .unwrap(),
            });
        }

        let encoder = OpusEncoderWrapper { ptr: encoder_ptr };

        let error = opus_encoder_ctl(encoder_ptr, OPUS_GET_LOOKAHEAD_REQUEST, lookahead as *mut _);
        if error != 0 {
            bail!(crate::Error::OpusEncode {
                reason: std::ffi::CStr::from_ptr(opus_strerror(error))
                    .to_str()
                    .unwrap(),
            });
        }
        encoder
    };

    write_header(writer, channels, *lookahead as usize)?;
    write_tags(writer)?;

    Ok(encoder)
}
