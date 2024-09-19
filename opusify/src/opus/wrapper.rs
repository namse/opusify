use opusic_sys::*;

pub struct OpusEncoderWrapper {
    ptr: *mut OpusEncoder,
}

impl OpusEncoderWrapper {
    pub fn new(channels: usize, sample_rate: usize) -> Result<Self, crate::Error> {
        unsafe {
            let mut error = 0;
            let encoder_ptr = opus_encoder_create(
                sample_rate as _,
                channels as _,
                OPUS_APPLICATION_AUDIO,
                &mut error,
            );
            if error != 0 {
                return Err(crate::Error::OpusEncode {
                    reason: std::ffi::CStr::from_ptr(opus_strerror(error))
                        .to_str()
                        .unwrap(),
                });
            }

            Ok(OpusEncoderWrapper { ptr: encoder_ptr })
        }
    }

    // TODO: Directly write on &mut [u8] instead of allocating a Vec<u8>
    pub fn encode(&mut self, pcm: &[i16], frame_size: usize) -> Result<Vec<u8>, crate::Error> {
        let mut output_buffer: Vec<u8> = vec![0; 8192];

        let output_len = unsafe {
            let output_len = opus_encode(
                self.ptr,
                pcm.as_ptr(),
                frame_size as _,
                output_buffer.as_mut_ptr(),
                output_buffer.len() as _,
            );
            if output_len < 0 {
                let error = output_len;
                return Err(crate::Error::OpusEncode {
                    reason: std::ffi::CStr::from_ptr(opus_strerror(error))
                        .to_str()
                        .unwrap(),
                });
            }
            output_len
        } as usize;

        output_buffer.truncate(output_len);

        Ok(output_buffer)
    }

    pub fn lookahead(&self) -> Result<usize, crate::Error> {
        unsafe {
            let mut lookahead = 0;

            let error = opus_encoder_ctl(self.ptr, OPUS_GET_LOOKAHEAD_REQUEST, &mut lookahead);

            if error != 0 {
                return Err(crate::Error::OpusEncode {
                    reason: std::ffi::CStr::from_ptr(opus_strerror(error))
                        .to_str()
                        .unwrap(),
                });
            }

            Ok(lookahead)
        }
    }
}

impl Drop for OpusEncoderWrapper {
    fn drop(&mut self) {
        unsafe {
            opus_encoder_destroy(self.ptr);
        }
    }
}
