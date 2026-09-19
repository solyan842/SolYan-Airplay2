use airplay_core::error::Result;
use rubato::{
    Resampler as RubatoResampler, SincFixedIn, SincInterpolationParameters,
    SincInterpolationType, WindowFunction,
};
use tracing::{debug, info};

pub const DEFAULT_CHUNK_SIZE: usize = 1024;

const SINC_PARAMS: SincInterpolationParameters = SincInterpolationParameters {
    sinc_len: 512,
    f_cutoff: 0.95,
    interpolation: SincInterpolationType::Cubic,
    oversampling_factor: 256,
    window: WindowFunction::BlackmanHarris2,
};

pub struct Resampler {
    inner: SincFixedIn<f32>,
    channels: usize,
    source_rate: u32,
    target_rate: u32,
    base_ratio: f64,
    applied_ppm: f64,
}

impl Resampler {
    pub fn new(source_rate: u32, target_rate: u32, channels: u8) -> Result<Self> {
        Self::with_chunk_size(source_rate, target_rate, channels, DEFAULT_CHUNK_SIZE)
    }

    pub fn with_chunk_size(
        source_rate: u32,
        target_rate: u32,
        channels: u8,
        chunk_size: usize,
    ) -> Result<Self> {
        let resample_ratio = target_rate as f64 / source_rate as f64;
        let channels_usize = channels as usize;

        let mut inner = SincFixedIn::<f32>::new(
            resample_ratio,
            1.01,
            SINC_PARAMS,
            chunk_size,
            channels_usize,
        )
        .map_err(|e| {
            airplay_core::error::StreamingError::Encoding(format!(
                "Failed to create resampler: {}",
                e
            ))
        })?;

        let output_delay = inner.output_delay();
        info!(
            "Resampler created: {}Hz -> {}Hz, {} channels, delay: {} frames ({:.1}ms)",
            source_rate,
            target_rate,
            channels,
            output_delay,
            output_delay as f32 / target_rate as f32 * 1000.0
        );

        let input_frames_needed = inner.input_frames_next();
        let priming_chunks = (SINC_PARAMS.sinc_len / input_frames_needed).max(3);
        let silence_chunk: Vec<Vec<f32>> = (0..channels_usize)
            .map(|_| vec![0.0f32; input_frames_needed])
            .collect();

        for i in 0..priming_chunks {
            match inner.process(&silence_chunk, None) {
                Ok(_) => debug!("Priming chunk {}/{} processed", i + 1, priming_chunks),
                Err(e) => {
                    debug!("Error during resampler priming: {}", e);
                    break;
                }
            }
        }

        Ok(Self {
            inner,
            channels: channels_usize,
            source_rate,
            target_rate,
            base_ratio: resample_ratio,
            applied_ppm: 0.0,
        })
    }

    pub fn source_rate(&self) -> u32 { self.source_rate }
    pub fn target_rate(&self) -> u32 { self.target_rate }
    pub fn channels(&self) -> usize { self.channels }
    pub fn input_frames_next(&self) -> usize { self.inner.input_frames_next() }
    pub fn applied_ppm(&self) -> f64 { self.applied_ppm }

    /// Apply a tiny relative ratio correction for asynchronous source/sink clocks.
    /// Positive ppm => slightly more output per input frame.
    /// The transition is ramped across the next chunk to avoid audible steps.
    pub fn set_drift_ppm(&mut self, ppm: f64) -> Result<()> {
        let clamped = ppm.clamp(-500.0, 500.0);
        if (clamped - self.applied_ppm).abs() < 0.5 {
            return Ok(());
        }
        let ratio = self.base_ratio * (1.0 + clamped / 1_000_000.0);
        self.inner
            .set_resample_ratio(ratio, true)
            .map_err(|e| {
                airplay_core::error::StreamingError::Encoding(format!(
                    "Failed to adjust resample ratio to {:.3} ppm: {}",
                    clamped, e
                ))
            })?;
        self.applied_ppm = clamped;
        tracing::debug!("Adaptive resampler drift correction: {:.2} ppm", clamped);
        Ok(())
    }

    pub fn reset(&mut self) {
        self.inner.reset();
        self.applied_ppm = 0.0;
        let _ = self.inner.set_resample_ratio(self.base_ratio, false);

        let input_frames_needed = self.inner.input_frames_next();
        let priming_chunks = (SINC_PARAMS.sinc_len / input_frames_needed).max(3);
        let silence_chunk: Vec<Vec<f32>> = (0..self.channels)
            .map(|_| vec![0.0f32; input_frames_needed])
            .collect();
        for _ in 0..priming_chunks {
            let _ = self.inner.process(&silence_chunk, None);
        }
    }

    pub fn process(&mut self, samples: &[i16]) -> Result<Vec<i16>> {
        if samples.is_empty() {
            return Ok(Vec::new());
        }

        let num_frames = samples.len() / self.channels;
        let mut input_channels: Vec<Vec<f32>> = (0..self.channels)
            .map(|_| Vec::with_capacity(num_frames))
            .collect();

        for frame_idx in 0..num_frames {
            for ch in 0..self.channels {
                let sample = samples[frame_idx * self.channels + ch];
                input_channels[ch].push(sample as f32 / 32768.0);
            }
        }

        let resampled = self.process_f32(&input_channels)?;
        Ok(interleave_with_dither(&resampled))
    }

    pub fn process_f32(&mut self, channels: &[Vec<f32>]) -> Result<Vec<Vec<f32>>> {
        if channels.is_empty() || channels[0].is_empty() {
            return Ok(Vec::new());
        }

        self.inner.process(channels, None).map_err(|e| {
            airplay_core::error::StreamingError::Encoding(format!("Resampling error: {}", e))
                .into()
        })
    }
}

#[inline]
pub fn dither_to_i16(sample: f32) -> i16 {
    let rand1 = fastrand::f32() - 0.5;
    let rand2 = fastrand::f32() - 0.5;
    let tpdf_noise = (rand1 + rand2) / 32768.0;
    let dithered = sample + tpdf_noise;
    (dithered * 32767.0).clamp(-32768.0, 32767.0) as i16
}

pub fn interleave_with_dither(channels: &[Vec<f32>]) -> Vec<i16> {
    if channels.is_empty() || channels[0].is_empty() {
        return Vec::new();
    }

    let num_frames = channels[0].len();
    let num_channels = channels.len();
    let mut output = Vec::with_capacity(num_frames * num_channels);
    for frame_idx in 0..num_frames {
        for ch in 0..num_channels {
            output.push(dither_to_i16(channels[ch][frame_idx]));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiny_drift_adjustment_is_accepted() {
        let mut r = Resampler::new(48_000, 44_100, 2).unwrap();
        r.set_drift_ppm(125.0).unwrap();
        assert!((r.applied_ppm() - 125.0).abs() < 0.01);
        r.set_drift_ppm(-125.0).unwrap();
        assert!((r.applied_ppm() + 125.0).abs() < 0.01);
    }

    #[test]
    fn drift_is_clamped() {
        let mut r = Resampler::new(48_000, 44_100, 2).unwrap();
        r.set_drift_ppm(5000.0).unwrap();
        assert!((r.applied_ppm() - 500.0).abs() < 0.01);
    }
}
