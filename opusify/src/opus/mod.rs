//! Inspired by https://github.com/enzo1982/superfast#superfast-codecs

mod ogg;
mod wrapper;

use crate::{decoded_chunk::DecodedChunk, OUT_SAMPLE_RATE};
use std::{
    collections::BTreeMap,
    sync::mpsc::{self},
};
use wrapper::*;

// it's okay to use a constant here because it has only one stream
const SERIAL: u32 = 12345;
// const FRAME_SIZE: usize = 2880;
const FRAME_SIZE: usize = 480;

pub fn encode_to_ogg_opus(
    in_rx: mpsc::Receiver<DecodedChunk>,
    err_tx: mpsc::Sender<crate::Error>,
) -> anyhow::Result<mpsc::Receiver<bytes::Bytes>> {
    let first_chunk = in_rx.recv()?;

    let channels = first_chunk.channels;

    let (encoded_tx, encoded_rx) = mpsc::channel();
    start_spawner_thread(in_rx, encoded_tx, first_chunk);
    let out_rx = start_ogg_writer_thread(encoded_rx, channels);

    Ok(out_rx)
}

fn padding_frames() -> usize {
    std::env::var("PADDING_FRAMES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8)
}

fn middle_frames() -> usize {
    std::env::var("MIDDLE_FRAMES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(96)
}

fn start_spawner_thread(
    in_rx: mpsc::Receiver<DecodedChunk>,
    encoded_tx: mpsc::Sender<Encoded>,
    first_chunk: DecodedChunk,
) {
    std::thread::spawn(move || {
        let now = std::time::Instant::now();
        let channels = first_chunk.channels;

        let left_padding_frames = padding_frames();
        let middle_frames = middle_frames();
        let right_padding_frames = padding_frames();

        let expected_encode_pcm_len =
            (left_padding_frames + middle_frames + right_padding_frames) * channels * FRAME_SIZE;
        let mut pcms: Vec<i16> = Vec::with_capacity(expected_encode_pcm_len);
        pcms.extend(first_chunk.pcm);

        let mut is_first = true;
        let mut sequence_number = 0;
        loop {
            while pcms.len() < expected_encode_pcm_len {
                let Ok(encoded) = in_rx.recv() else {
                    break;
                };
                pcms.extend(encoded.pcm);
            }

            let encode_pcm_len = pcms.len().min(expected_encode_pcm_len);
            let is_end = encode_pcm_len < expected_encode_pcm_len;

            if is_end {
                pcms.resize(expected_encode_pcm_len, 0);
            }

            assert!(pcms.len() >= expected_encode_pcm_len);

            spawn_encoding_job(
                encoded_tx.clone(),
                EncodingRequest {
                    kind: match (is_first, is_end) {
                        (true, true) => EncodingRequestKind::FirstAndEnd,
                        (true, false) => EncodingRequestKind::First,
                        (false, true) => EncodingRequestKind::End,
                        (false, false) => EncodingRequestKind::Middle,
                    },
                    frame_size: FRAME_SIZE,
                    pcm: pcms[..expected_encode_pcm_len].to_vec(),
                    channels,
                    sequence_number,
                },
            );
            sequence_number += 1;

            if is_end {
                break;
            }

            let removal_pcm_len = encode_pcm_len
                - (left_padding_frames + right_padding_frames) * channels * FRAME_SIZE;
            pcms.drain(..removal_pcm_len);

            is_first = false;
        }

        println!("spawner thread finished, elapsed: {:?}", now.elapsed());
    });
}

struct EncodingRequest {
    kind: EncodingRequestKind,
    frame_size: usize,
    /// (left_padding_frames + middle_frames + right_padding_frames) * channels * FRAME_SIZE
    pcm: Vec<i16>,
    channels: usize,
    sequence_number: usize,
}

#[derive(Debug)]
enum EncodingRequestKind {
    First,
    Middle,
    End,
    FirstAndEnd,
}

fn spawn_encoding_job(encoded_tx: mpsc::Sender<Encoded>, request: EncodingRequest) {
    rayon::spawn_fifo(move || {
        let result: anyhow::Result<()> = (|| {
            let mut encoder = OpusEncoderWrapper::new(request.channels, OUT_SAMPLE_RATE)?;

            let left_padding_frames = padding_frames();
            let middle_frames = middle_frames();

            let frame_pcm_len = request.channels * request.frame_size;

            let mut packets = Vec::with_capacity(middle_frames);

            for (frame_index, chunk) in request.pcm.chunks(frame_pcm_len).enumerate() {
                if left_padding_frames + middle_frames <= frame_index {
                    if let EncodingRequestKind::First | EncodingRequestKind::Middle = request.kind {
                        break;
                    }
                }

                let output = encoder.encode(chunk, request.frame_size)?;

                if frame_index < left_padding_frames {
                    if let EncodingRequestKind::Middle | EncodingRequestKind::End = request.kind {
                        continue;
                    }
                }

                packets.push(OpusPacket {
                    data: output,
                    frame_size: request.frame_size,
                });
            }

            encoded_tx.send(Encoded {
                sequence_number: request.sequence_number,
                packets,
            })?;

            Ok(())
        })();
    });
}

struct Encoded {
    sequence_number: SequenceNumber,
    packets: OpusPackets,
}

type SequenceNumber = usize;
type OpusPackets = Vec<OpusPacket>;

struct OpusPacket {
    data: Vec<u8>,
    frame_size: usize,
}

fn start_ogg_writer_thread(
    encoded_rx: mpsc::Receiver<Encoded>,
    channels: usize,
) -> mpsc::Receiver<bytes::Bytes> {
    let (out_tx, out_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let now = std::time::Instant::now();

        let _result: anyhow::Result<()> = (move || {
            let mut writer = ogg::PacketWriter::new(out_tx);
            let lookahead = OpusEncoderWrapper::new(channels, OUT_SAMPLE_RATE)?.lookahead()?;

            write_header(&mut writer, channels, lookahead)?;
            write_tags(&mut writer)?;

            let mut sample_acc = 0;

            let mut queue = BTreeMap::<SequenceNumber, OpusPackets>::new();
            let mut next_sequence_number = 0;
            let mut last_encoded = None;

            while let Ok(encoded) = encoded_rx.recv() {
                // Keep the last packet to know when it's the end of the stream
                let Some(encoded) = std::mem::replace(&mut last_encoded, Some(encoded)) else {
                    continue;
                };
                queue.insert(encoded.sequence_number, encoded.packets);

                while let Some(packets) = queue.remove(&next_sequence_number) {
                    handle_packets(&mut writer, packets, lookahead, &mut sample_acc, false)?;
                    next_sequence_number += 1;
                }
            }

            if let Some(encoded) = last_encoded {
                queue.insert(encoded.sequence_number, encoded.packets);
            }

            while let Some(packets) = queue.remove(&next_sequence_number) {
                handle_packets(
                    &mut writer,
                    packets,
                    lookahead,
                    &mut sample_acc,
                    queue.is_empty(),
                )?;
                next_sequence_number += 1;
            }

            println!("ogg writer thread finished, elapsed: {:?}", now.elapsed());

            Ok(())
        })();
    });

    out_rx
}

fn handle_packets(
    writer: &mut ogg::PacketWriter,
    packets: OpusPackets,
    lookahead: usize,
    sample_acc: &mut usize,
    is_end: bool,
) -> anyhow::Result<()> {
    for packet in packets {
        write_opus_packet_to_ogg(writer, packet, lookahead, sample_acc, is_end)?;
    }
    Ok(())
}

fn write_opus_packet_to_ogg(
    writer: &mut ogg::PacketWriter,
    packet: OpusPacket,
    lookahead: usize,
    sample_acc: &mut usize,
    is_end: bool,
) -> anyhow::Result<()> {
    *sample_acc += packet.frame_size;

    // https://wiki.xiph.org/OggOpus#Granule_Position
    let granule_position = lookahead + *sample_acc;

    writer.write_packet(
        packet.data,
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
