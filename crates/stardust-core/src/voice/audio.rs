use super::VoiceError;

pub const SAMPLE_RATE: u32 = 16_000;
pub const MAX_RECORDING_SECONDS: u32 = 10;
const HEADER_BYTES: usize = 44;
const SAMPLE_BYTES: usize = size_of::<i16>();
pub const MAX_WAV_BYTES: usize =
    HEADER_BYTES + SAMPLE_BYTES * (SAMPLE_RATE * MAX_RECORDING_SECONDS) as usize;

pub(super) fn sample_limit(rate: u32) -> Result<usize, VoiceError> {
    if !(8_000..=192_000).contains(&rate) {
        return Err(VoiceError::InvalidAudio);
    }
    Ok((rate * MAX_RECORDING_SECONDS) as usize)
}

fn check_samples(samples: &[i16], rate: u32) -> Result<(), VoiceError> {
    let limit = sample_limit(rate)?;
    if samples.is_empty() {
        return Err(VoiceError::InvalidAudio);
    }
    if samples.len() > limit {
        return Err(VoiceError::RecordingTooLong);
    }
    Ok(())
}

/// Linearly interpolate a bounded mono PCM16 clip at 8–192 kHz to 16 kHz.
/// The last input sample is held through the end of its sample period.
pub fn resample_to_16khz(samples: &[i16], rate: u32) -> Result<Vec<i16>, VoiceError> {
    check_samples(samples, rate)?;
    if rate == SAMPLE_RATE {
        return Ok(samples.to_vec());
    }
    let output_rate = u64::from(SAMPLE_RATE);
    let output_count = (samples.len() as u64 * output_rate).div_ceil(u64::from(rate));
    let mut output = Vec::with_capacity(output_count as usize);
    for index in 0..output_count {
        let position = index * u64::from(rate);
        let left = (position / output_rate) as usize;
        let right = (left + 1).min(samples.len() - 1);
        let weight = (position % output_rate) as i32;
        let value = i32::from(samples[left]) * (SAMPLE_RATE as i32 - weight)
            + i32::from(samples[right]) * weight;
        output.push((value / SAMPLE_RATE as i32) as i16);
    }
    Ok(output)
}

/// Encode nonempty 16 kHz mono PCM16 as a canonical RIFF/WAVE body.
pub fn encode_wav(samples: &[i16]) -> Result<Vec<u8>, VoiceError> {
    check_samples(samples, SAMPLE_RATE)?;
    let data_bytes = samples.len() * SAMPLE_BYTES;
    let mut wav = Vec::with_capacity(HEADER_BYTES + data_bytes);
    wav.extend_from_slice(&header(data_bytes as u32));
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(wav)
}

pub(super) fn validate_wav(wav: &[u8]) -> Result<(), VoiceError> {
    if wav.len() > MAX_WAV_BYTES {
        return Err(VoiceError::RecordingTooLong);
    }
    if wav.len() <= HEADER_BYTES || !wav.len().is_multiple_of(SAMPLE_BYTES) {
        return Err(VoiceError::InvalidAudio);
    }
    if wav[..HEADER_BYTES] != header((wav.len() - HEADER_BYTES) as u32) {
        return Err(VoiceError::InvalidAudio);
    }
    Ok(())
}

fn header(data_bytes: u32) -> [u8; HEADER_BYTES] {
    let mut header =
        *b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\0\0\0\0\0\0\0\0\x02\0\x10\0data\0\0\0\0";
    header[4..8].copy_from_slice(&(data_bytes + 36).to_le_bytes());
    header[24..28].copy_from_slice(&SAMPLE_RATE.to_le_bytes());
    header[28..32].copy_from_slice(&(SAMPLE_RATE * SAMPLE_BYTES as u32).to_le_bytes());
    header[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    header
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_and_resampling_obey_pcm_and_duration_bounds() {
        assert_eq!(encode_wav(&[i16::MIN, 0, i16::MAX]).unwrap(), b"RIFF\x2a\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x80\x3e\0\0\0\x7d\0\0\x02\0\x10\0data\x06\0\0\0\0\x80\0\0\xff\x7f");
        assert_eq!(
            resample_to_16khz(&[-100, 100], 8_000).unwrap(),
            [-100, 0, 100, 100]
        );
        assert_eq!(resample_to_16khz(&[0, 1, 2, 3], 32_000).unwrap(), [0, 2]);
        assert_eq!(resample_to_16khz(&[123], 8_000).unwrap(), [123, 123]);
        assert_eq!(resample_to_16khz(&[1], 0), Err(VoiceError::InvalidAudio));
        assert_eq!(
            resample_to_16khz(&[], SAMPLE_RATE),
            Err(VoiceError::InvalidAudio)
        );
        let max = (SAMPLE_RATE * MAX_RECORDING_SECONDS) as usize;
        assert!(encode_wav(&vec![0; max]).is_ok());
        assert_eq!(
            encode_wav(&vec![0; max + 1]),
            Err(VoiceError::RecordingTooLong)
        );
        assert_eq!(
            resample_to_16khz(&vec![0; max + 1], SAMPLE_RATE),
            Err(VoiceError::RecordingTooLong)
        );
    }
}
