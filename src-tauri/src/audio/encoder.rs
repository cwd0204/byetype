/// 内部统一的采样率：采集、FLAC 编码与转 WAV 都用这个。
pub const SAMPLE_RATE: u32 = 16_000;

/// Encode PCM i16 samples into a FLAC byte buffer.
/// Lossless compression, typically ~50% smaller than WAV.
pub fn encode_flac(samples: &[i16]) -> Result<Vec<u8>, String> {
    encode_flac_inner(samples, None)
}

pub fn encode_flac_with_cancel(
    samples: &[i16],
    cancellation: tokio_util::sync::CancellationToken,
) -> Result<Vec<u8>, String> {
    encode_flac_inner(samples, Some(cancellation))
}

fn encode_flac_inner(
    samples: &[i16],
    cancellation: Option<tokio_util::sync::CancellationToken>,
) -> Result<Vec<u8>, String> {
    use flacenc::bitsink::{BitSink, Bits, ByteSink};
    use flacenc::component::BitRepr;
    use flacenc::config;
    use flacenc::error::{SourceError, Verify};
    use flacenc::source::{Fill, MemSource, Source};

    struct CancellableSource {
        source: MemSource,
        cancellation: Option<tokio_util::sync::CancellationToken>,
    }

    struct CancellableSink {
        sink: ByteSink,
        cancellation: Option<tokio_util::sync::CancellationToken>,
    }

    impl CancellableSink {
        fn ensure_active(&self) -> Result<(), std::io::Error> {
            if self
                .cancellation
                .as_ref()
                .is_some_and(|token| token.is_cancelled())
            {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "request cancelled",
                ))
            } else {
                Ok(())
            }
        }
    }

    impl BitSink for CancellableSink {
        type Error = std::io::Error;

        fn align_to_byte(&mut self) -> Result<usize, Self::Error> {
            self.ensure_active()?;
            Ok(self.sink.align_to_byte().unwrap())
        }

        fn write_lsbs<T: Bits>(&mut self, value: T, bits: usize) -> Result<(), Self::Error> {
            self.ensure_active()?;
            Ok(self.sink.write_lsbs(value, bits).unwrap())
        }

        fn write_msbs<T: Bits>(&mut self, value: T, bits: usize) -> Result<(), Self::Error> {
            self.ensure_active()?;
            Ok(self.sink.write_msbs(value, bits).unwrap())
        }

        fn write<T: Bits>(&mut self, value: T) -> Result<(), Self::Error> {
            self.ensure_active()?;
            Ok(self.sink.write(value).unwrap())
        }
    }

    impl Source for CancellableSource {
        fn channels(&self) -> usize {
            self.source.channels()
        }

        fn bits_per_sample(&self) -> usize {
            self.source.bits_per_sample()
        }

        fn sample_rate(&self) -> usize {
            self.source.sample_rate()
        }

        fn read_samples<F: Fill>(
            &mut self,
            block_size: usize,
            dest: &mut F,
        ) -> Result<usize, SourceError> {
            if self
                .cancellation
                .as_ref()
                .is_some_and(|token| token.is_cancelled())
            {
                return Err(SourceError::from_io_error(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "request cancelled",
                )));
            }
            self.source.read_samples(block_size, dest)
        }

        fn len_hint(&self) -> Option<usize> {
            self.source.len_hint()
        }
    }

    let mut samples_i32 = Vec::with_capacity(samples.len());
    for chunk in samples.chunks(4096) {
        if cancellation
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
        {
            return Err("请求已取消".to_string());
        }
        samples_i32.extend(chunk.iter().map(|&sample| sample as i32));
    }
    let source = CancellableSource {
        source: MemSource::from_samples(&samples_i32, 1, 16, SAMPLE_RATE as usize),
        cancellation: cancellation.clone(),
    };
    let encoder_config = config::Encoder::default()
        .into_verified()
        .map_err(|e| format!("FLAC config error: {:?}", e))?;
    let flac_stream =
        flacenc::encode_with_fixed_block_size(&encoder_config, source, encoder_config.block_size)
            .map_err(|error| {
            if cancellation
                .as_ref()
                .is_some_and(|token| token.is_cancelled())
            {
                "请求已取消".to_string()
            } else {
                format!("FLAC encode error: {:?}", error)
            }
        })?;

    if cancellation
        .as_ref()
        .is_some_and(|token| token.is_cancelled())
    {
        return Err("请求已取消".to_string());
    }

    let mut sink = CancellableSink {
        sink: ByteSink::new(),
        cancellation: cancellation.clone(),
    };
    flac_stream.write(&mut sink).map_err(|error| {
        if cancellation
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
        {
            "请求已取消".to_string()
        } else {
            format!("FLAC write error: {:?}", error)
        }
    })?;
    Ok(sink.sink.into_inner())
}

/// Encode bytes to Base64 string.
pub fn audio_to_base64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// 给 16 位单声道小端 PCM 套一个 44 字节的标准 WAV 头。
///
/// Bedrock 上的 Voxtral 只接受 `mp3` 与 `wav`，不认我们内部用的 FLAC，所以那条路径要在
/// 发送前转成 WAV。这里只补头不重采样，输入必须已经是单声道 16 位小端。
pub fn wrap_pcm16_as_wav(pcm_le_bytes: &[u8], sample_rate: u32) -> Vec<u8> {
    let data_len = pcm_le_bytes.len() as u32;
    let mut wav = Vec::with_capacity(44 + pcm_le_bytes.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk 长度
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // 单声道
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // 字节率 = 采样率 × 2 字节
    wav.extend_from_slice(&2u16.to_le_bytes()); // 每帧字节数
    wav.extend_from_slice(&16u16.to_le_bytes()); // 位深
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm_le_bytes);
    wav
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_canonical() {
        let pcm: Vec<u8> = (0i16..4).flat_map(|s| s.to_le_bytes()).collect();
        let wav = wrap_pcm16_as_wav(&pcm, SAMPLE_RATE);
        assert_eq!(wav.len(), 44 + pcm.len());
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        // RIFF 长度 = 文件长度 - 8
        assert_eq!(
            u32::from_le_bytes(wav[4..8].try_into().unwrap()),
            (wav.len() - 8) as u32
        );
        assert_eq!(u16::from_le_bytes(wav[20..22].try_into().unwrap()), 1); // PCM
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1); // 单声道
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), SAMPLE_RATE);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16); // 位深
        assert_eq!(
            u32::from_le_bytes(wav[40..44].try_into().unwrap()),
            pcm.len() as u32
        );
        assert_eq!(&wav[44..], &pcm[..]);
    }

    #[test]
    fn wav_header_handles_empty_pcm() {
        let wav = wrap_pcm16_as_wav(&[], SAMPLE_RATE);
        assert_eq!(wav.len(), 44);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 0);
    }
}
