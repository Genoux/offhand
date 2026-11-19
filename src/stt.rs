use anyhow::{Context, Result};
use parakeet_rs::{ExecutionConfig, Nemotron};
use std::path::Path;
use std::time::Instant;

pub struct Stt {
    model: Nemotron,
}

impl Stt {
    pub fn load(dir: &Path) -> Result<Self> {
        let started = Instant::now();
        // Prepacking copies every weight into the heap in a kernel-friendly
        // layout: 2.3GB of anonymous memory that only swap could reclaim, which
        // made the idle daemon earlyoom's first target. Without it the weights
        // stay mmap'd from encoder.onnx.data, so idle anonymous memory is 168MB
        // and the kernel can evict the rest under pressure. Measured on a 20s
        // clip, 16 cores: 4.0s -> 5.1-5.5s offline, load 2.6s -> 0.7s,
        // identical transcript.
        let config = ExecutionConfig::new()
            .with_custom_configure(|builder| Ok(builder.with_prepacking(false)?));
        let model = Nemotron::from_pretrained(dir, Some(config))
            .with_context(|| format!("loading speech model from {}", dir.display()))?;
        eprintln!(
            "speech model loaded in {:?} (nemotron, cpu, {}ms streaming chunks)",
            started.elapsed(),
            model.chunk_samples() * 1000 / crate::audio::SAMPLE_RATE as usize
        );
        Ok(Self { model })
    }

    pub fn chunk_samples(&self) -> usize {
        self.model.chunk_samples()
    }

    pub fn start_stream(&mut self) {
        self.model.reset();
    }

    pub fn stream_chunk(&mut self, audio: &[f32]) -> Result<()> {
        anyhow::ensure!(
            audio.len() == self.chunk_samples(),
            "incorrect streaming chunk size"
        );
        self.model.transcribe_chunk(audio)?;
        Ok(())
    }

    pub fn finish_stream(&mut self, audio: &[f32]) -> Result<String> {
        let size = self.chunk_samples();
        let mut chunks = audio.chunks_exact(size);
        for chunk in &mut chunks {
            self.stream_chunk(chunk)?;
        }
        let remainder = chunks.remainder();
        if !remainder.is_empty() {
            // parakeet-rs 0.3.7 has no flush API and decodes only full chunks.
            let mut padded = vec![0.0; size];
            padded[..remainder.len()].copy_from_slice(remainder);
            self.stream_chunk(&padded)?;
        }
        // The decoder emits a word only once it has heard past it, so a release
        // right after the last word dropped that word: 3 of 8 recordings trimmed
        // to their speech lost it ("let's keep it" came back "let's keep"), and
        // one silent chunk recovered all of them (examples measured on
        // parakeet-rs 0.3.7, which has no flush API to do this instead).
        self.stream_chunk(&vec![0.0; size])?;
        Ok(self.model.get_transcript().trim().to_string())
    }

    pub fn transcribe(&mut self, mut audio: Vec<f32>) -> Result<String> {
        audio.resize(audio.len() + self.chunk_samples(), 0.0);
        self.model.reset();
        Ok(self.model.transcribe_audio(&audio)?.trim().to_string())
    }

    /// Hand the weight pages a dictation faulted in back to the page cache.
    ///
    /// Without prepacking the weights are a file mapping, and after one
    /// dictation all 2.3GB of it counts in this process's RSS. That memory is
    /// reclaimable, but earlyoom ranks by RSS and picks the daemon anyway.
    /// `MADV_DONTNEED` only drops this process's page table entries: the pages
    /// stay cached, so the next dictation re-faults them without reading disk
    /// unless the kernel needed the memory in between.
    #[cfg(target_os = "linux")]
    pub fn release_weights(&self) {
        let Ok(smaps) = std::fs::read_to_string("/proc/self/smaps") else {
            return;
        };
        for (start, end) in untouched_weight_mappings(&smaps) {
            // SAFETY: the range is a whole mapping the kernel just listed with no
            // anonymous pages, so DONTNEED discards nothing that is not on disk.
            unsafe { libc::madvise(start as *mut libc::c_void, end - start, libc::MADV_DONTNEED) };
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn release_weights(&self) {}
}

/// Weight mappings this process never wrote to.
///
/// ONNX Runtime maps `encoder.onnx.data` private and writable (`rw-p`), so the
/// permissions prove nothing: DONTNEED on a private page it had written would
/// throw the write away and re-read the file. `Anonymous: 0 kB` is the kernel
/// saying no page was ever copied on write.
#[cfg(target_os = "linux")]
fn untouched_weight_mappings(smaps: &str) -> Vec<(usize, usize)> {
    let mut mappings = Vec::new();
    let mut current = None;
    for line in smaps.lines() {
        let first = line.split_whitespace().next().unwrap_or_default();
        if !first.ends_with(':') {
            current = line
                .ends_with(".onnx.data")
                .then(|| first.split_once('-'))
                .flatten()
                .and_then(|(start, end)| {
                    Some((
                        usize::from_str_radix(start, 16).ok()?,
                        usize::from_str_radix(end, 16).ok()?,
                    ))
                });
        } else if let Some(range) = current
            && first == "Anonymous:"
        {
            if line.split_whitespace().nth(1) == Some("0") {
                mappings.push(range);
            }
            current = None;
        }
    }
    mappings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_model_directory_is_a_clear_error_not_a_panic() {
        let dir =
            std::env::temp_dir().join(format!("offhand-no-such-model-{}", std::process::id()));
        let Err(err) = Stt::load(&dir) else {
            panic!("a missing model directory must not load successfully");
        };
        assert!(err.to_string().contains("loading speech model"), "{err}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn only_weight_mappings_never_written_to_are_released() {
        let smaps = "\
7f00a000-7f00c000 rw-p 00000000 103:02 42 /m/nemotron/encoder.onnx.data
Rss:                   8 kB
Anonymous:             0 kB
VmFlags: rd wr mr mw me ac
7f00c000-7f00e000 rw-p 00002000 103:02 42 /m/nemotron/encoder.onnx.data
Anonymous:             4 kB
7f00e000-7f010000 r--p 00000000 103:02 43 /usr/lib/libc.so.6
Anonymous:             0 kB
7f010000-7f012000 rw-p 00000000 00:00 0
Anonymous:             0 kB
";
        assert_eq!(
            untouched_weight_mappings(smaps),
            vec![(0x7f00a000, 0x7f00c000)]
        );
    }
}
