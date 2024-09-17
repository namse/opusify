use crate::{decoded_chunk::DecodedChunk, libswresample_bindings::*};
use anyhow::*;
use futures::TryStream;

macro_rules! check_error {
    ($e:expr) => {
        let error = $e;
        if error < 0 {
            let errbuf_size = 256;
            let mut errbuf = [0; 256];
            assert_eq!(av_strerror(error, errbuf.as_mut_ptr(), errbuf_size), 0);
            let errstr = std::ffi::CStr::from_ptr(errbuf.as_ptr()).to_str()?;
            return Err(anyhow!("{}", errstr));
        }
    };
}

pub fn resample(stream: impl TryStream<Ok = DecodedChunk, Error = Error>) -> Result<()> {
    // Maybe you want to get this docs https://www.ffmpeg.org/doxygen/7.0/group__lswr.html

    let channel = 2;
    let in_sample_rate = 44100;
    unsafe {
        let mut ctx = std::ptr::null_mut();
        let mut channel_layout: AVChannelLayout = std::mem::zeroed();
        av_channel_layout_default(&mut channel_layout, channel as _);

        let error = swr_alloc_set_opts2(
            &mut ctx,
            &channel_layout,
            AVSampleFormat_AV_SAMPLE_FMT_S16,
            48000,
            &channel_layout,
            AVSampleFormat_AV_SAMPLE_FMT_S16,
            in_sample_rate,
            0,
            std::ptr::null_mut(),
        );
        check_error!(error);

        let ctx = SwrContextWrapper { ptr: ctx };

        let error = swr_init(ctx.ptr);
        check_error!(error);

        // swr_convert(ctx.ptr, out, out_count, in_, in_count);
    }

    todo!()
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
