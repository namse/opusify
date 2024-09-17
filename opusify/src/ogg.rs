// Forked from https://github.com/RustAudio/ogg/blob/4b22c17d4ac365e8f4e16c69f0e906a95731e781/src/writing.rs
// Please check ogg.LICENSE

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::mpsc::{SendError, SyncSender};

pub struct PacketWriter<'writer> {
    tx: SyncSender<bytes::Bytes>,
    page_vals: HashMap<u32, CurrentPageValues<'writer>>,
}

struct CurrentPageValues<'writer> {
    /// `true` if this page is the first one in the logical bitstream
    first_page: bool,
    /// Page counter of the current page
    /// Increased for every page
    sequence_num: u32,

    /// Points to the first unwritten position in cur_pg_lacing.
    segment_cnt: u8,
    cur_pg_lacing: [u8; 255],
    /// The data and the absgp's of the packets
    cur_pg_data: Vec<(Cow<'writer, [u8]>, u64)>,

    /// Some(offs), if the last packet
    /// couldn't make it fully into this page, and
    /// has to be continued in the next page.
    ///
    /// `offs` should point to the first idx in
    /// cur_pg_data[last] that should NOT be written
    /// in this page anymore.
    ///
    /// None if all packets can be written nicely.
    pck_this_overflow_idx: Option<usize>,

    /// Some(offs), if the first packet
    /// couldn't make it fully into the last page, and
    /// has to be continued in this page.
    ///
    /// `offs` should point to the first idx in cur_pg_data[0]
    /// that hasn't been written.
    ///
    /// None if all packets can be written nicely.
    pck_last_overflow_idx: Option<usize>,
}

/// Specifies whether to end something with the write of the packet.
///
/// If you want to end a stream you need to inform the Ogg `PacketWriter`
/// about this. This is the enum to do so.
///
/// Also, Codecs sometimes have special requirements to put
/// the first packet of the whole stream into its own page.
/// The `EndPage` variant can be used for this.
#[derive(PartialEq, Clone, Copy)]
pub enum PacketWriteEndInfo {
    /// No ends here, just a normal packet
    NormalPacket,
    /// Force-end the current page
    EndPage,
    /// End the whole logical stream.
    EndStream,
}

impl<'writer> PacketWriter<'writer> {
    pub fn new(tx: SyncSender<bytes::Bytes>) -> PacketWriter<'writer> {
        PacketWriter {
            tx,
            page_vals: HashMap::new(),
        }
    }
    pub fn write_packet<P: Into<Cow<'writer, [u8]>>>(
        &mut self,
        pck_cont: P,
        serial: u32,
        inf: PacketWriteEndInfo,
        /* TODO find a better way to design the API around
            passing the absgp to the underlying implementation.
            e.g. the caller passes a closure on init which gets
            called when we encounter a new page... with the param
            the index inside the current page, or something.
        */
        absgp: u64,
    ) -> Result<(), SendError<bytes::Bytes>> {
        let is_end_stream: bool = inf == PacketWriteEndInfo::EndStream;
        let pg = self.page_vals.entry(serial).or_insert(CurrentPageValues {
            first_page: true,
            sequence_num: 0,
            segment_cnt: 0,
            cur_pg_lacing: [0; 255],
            cur_pg_data: Vec::with_capacity(255),
            pck_this_overflow_idx: None,
            pck_last_overflow_idx: None,
        });

        let pck_cont = pck_cont.into();
        let cont_len = pck_cont.len();
        pg.cur_pg_data.push((pck_cont, absgp));

        let last_data_segment_size = (cont_len % 255) as u8;
        let needed_segments: usize = (cont_len / 255) + 1;
        let mut segment_in_page_i: u8 = pg.segment_cnt;
        let mut at_page_end: bool = false;
        for segment_i in 0..needed_segments {
            at_page_end = false;
            if segment_i + 1 < needed_segments {
                // For all segments containing 255 pieces of data
                pg.cur_pg_lacing[segment_in_page_i as usize] = 255;
            } else {
                // For the last segment, must contain < 255 pieces of data
                // (including 0)
                pg.cur_pg_lacing[segment_in_page_i as usize] = last_data_segment_size;
            }
            pg.segment_cnt = segment_in_page_i + 1;
            segment_in_page_i = (segment_in_page_i + 1) % 255;
            if segment_in_page_i == 0 {
                if segment_i + 1 < needed_segments {
                    // We have to flush a page, but we know there are more to come...
                    pg.pck_this_overflow_idx = Some((segment_i + 1) * 255);
                    PacketWriter::write_page(&self.tx, serial, pg, false)?;
                } else {
                    // We have to write a page end, and it's the very last
                    // we need to write
                    PacketWriter::write_page(&self.tx, serial, pg, is_end_stream)?;
                    // Not actually required
                    // (it is always None except if we set it to Some directly
                    // before we call write_page)
                    pg.pck_this_overflow_idx = None;
                    // Required (it could have been Some(offs) before)
                    pg.pck_last_overflow_idx = None;
                }
                at_page_end = true;
            }
        }
        if (inf != PacketWriteEndInfo::NormalPacket) && !at_page_end {
            // Write a page end
            PacketWriter::write_page(&self.tx, serial, pg, is_end_stream)?;

            pg.pck_last_overflow_idx = None;

            // TODO if inf was PacketWriteEndInfo::EndStream, we have to
            // somehow erase pg from the hashmap...
            // any ideas? perhaps needs external scope...
        }
        // All went fine.
        Ok(())
    }
    fn write_page(
        tx: &SyncSender<bytes::Bytes>,
        serial: u32,
        pg: &mut CurrentPageValues,
        last_page: bool,
    ) -> Result<(), SendError<bytes::Bytes>> {
        {
            // The page header with everything but the lacing values:
            let mut header: Vec<u8> = Vec::with_capacity(27);
            header.extend_from_slice(&[0x4f, 0x67, 0x67, 0x53, 0x00]);

            let mut flags: u8 = 0;
            if pg.pck_last_overflow_idx.is_some() {
                flags |= 0x01;
            }
            if pg.first_page {
                flags |= 0x02;
            }
            if last_page {
                flags |= 0x04;
            }

            header.push(flags);

            let pck_data = &pg.cur_pg_data;

            let mut last_finishing_pck_absgp: u64 = (-1i64) as u64;
            for (idx, &(_, absgp)) in pck_data.iter().enumerate() {
                if !(idx + 1 == pck_data.len() && pg.pck_this_overflow_idx.is_some()) {
                    last_finishing_pck_absgp = absgp;
                }
            }

            header.extend_from_slice(&last_finishing_pck_absgp.to_le_bytes());
            header.extend_from_slice(&serial.to_le_bytes());
            header.extend_from_slice(&pg.sequence_num.to_le_bytes());

            // checksum, calculated later on :)
            // tri!(header.write_u32::<LittleEndian>(0));
            header.extend_from_slice(&(0u32).to_le_bytes());

            header.push(pg.segment_cnt);

            let mut hash_calculated: u32;

            let pg_lacing = &pg.cur_pg_lacing[0..pg.segment_cnt as usize];

            hash_calculated = vorbis_crc32_update(0, header.as_ref());
            hash_calculated = vorbis_crc32_update(hash_calculated, pg_lacing);

            for (idx, (pck, _)) in pck_data.iter().enumerate() {
                let mut start: usize = 0;
                if idx == 0 {
                    if let Some(idx) = pg.pck_last_overflow_idx {
                        start = idx;
                    }
                }
                let mut end: usize = pck.len();
                if idx + 1 == pck_data.len() {
                    if let Some(idx) = pg.pck_this_overflow_idx {
                        end = idx;
                    }
                }
                hash_calculated = vorbis_crc32_update(hash_calculated, &pck[start..end]);
            }

            // Go back to enter the checksum
            // Don't do excessive checking here (that the seek
            // succeeded & we are at the right pos now).
            // It's hopefully not required.
            header[22..26].copy_from_slice(&hash_calculated.to_le_bytes());

            // Now all is done, write the stuff!
            tx.send(bytes::Bytes::from(header))?;
            tx.send(bytes::Bytes::copy_from_slice(&pg_lacing))?;
            for (idx, (pck, _)) in pck_data.iter().enumerate() {
                let mut start: usize = 0;
                if idx == 0 {
                    if let Some(idx) = pg.pck_last_overflow_idx {
                        start = idx;
                    }
                }
                let mut end: usize = pck.len();
                if idx + 1 == pck_data.len() {
                    if let Some(idx) = pg.pck_this_overflow_idx {
                        end = idx;
                    }
                }
                tx.send(bytes::Bytes::copy_from_slice(&pck[start..end]))?;
            }
        }

        // Reset the page.
        pg.first_page = false;
        pg.sequence_num += 1;

        pg.segment_cnt = 0;
        // If we couldn't fully write the last
        // packet, we need to keep it for the next page,
        // otherwise just clear everything.
        if pg.pck_this_overflow_idx.is_some() {
            let d = pg.cur_pg_data.pop().unwrap();
            pg.cur_pg_data.clear();
            pg.cur_pg_data.push(d);
        } else {
            pg.cur_pg_data.clear();
        }

        pg.pck_last_overflow_idx = pg.pck_this_overflow_idx;
        pg.pck_this_overflow_idx = None;

        Ok(())
    }
}

static CRC_LOOKUP_ARRAY: &[u32] = &lookup_array();

const fn get_tbl_elem(idx: u32) -> u32 {
    let mut r: u32 = idx << 24;
    let mut i = 0;
    while i < 8 {
        r = (r << 1) ^ (-(((r >> 31) & 1) as i32) as u32 & 0x04c11db7);
        i += 1;
    }
    return r;
}

const fn lookup_array() -> [u32; 0x100] {
    let mut lup_arr: [u32; 0x100] = [0; 0x100];
    let mut i = 0;
    while i < 0x100 {
        lup_arr[i] = get_tbl_elem(i as u32);
        i += 1;
    }
    lup_arr
}

fn vorbis_crc32_update(cur: u32, array: &[u8]) -> u32 {
    let mut ret: u32 = cur;
    for av in array {
        ret = (ret << 8) ^ CRC_LOOKUP_ARRAY[(*av as u32 ^ (ret >> 24)) as usize];
    }
    ret
}
