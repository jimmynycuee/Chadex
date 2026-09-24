//! Cargo test execution-count evidence derived from complete harness summaries.

use crate::validation_evidence::{
    parse_complete_cargo_test_summary_counts, CargoTestCountEvidenceStatus,
};

/// A single output line is far smaller than this in canonical libtest output.
/// Bounding partial-line retention keeps the streaming accumulator independent
/// from arbitrary project output while failing closed if framing cannot be
/// proven.
const CARGO_TEST_STREAM_LINE_MAX_BYTES: usize = 16 * 1024;
const CARGO_TEST_RESULT_MARKER: &[u8] = b"test result:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CargoTestRunMetadata {
    pub tests_detected: bool,
    pub tests_run_count: Option<u64>,
    pub tests_passed: Option<u64>,
    pub tests_failed: Option<u64>,
    pub zero_tests_run: Option<bool>,
    pub count_evidence_status: CargoTestCountEvidenceStatus,
}

impl CargoTestRunMetadata {
    pub const fn count_evidence_reason(self) -> &'static str {
        self.count_evidence_status.reason_code()
    }
}

#[derive(Debug, Default)]
struct CargoTestAggregate {
    tests_run_count: u64,
    tests_passed: u64,
    tests_failed: u64,
    complete_summary_found: bool,
    incomplete_summary_found: bool,
    tests_detected: bool,
    incomplete_stream: bool,
}

impl CargoTestAggregate {
    fn observe_line(&mut self, line: &str) {
        let line = line.trim_end_matches('\r');
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("running ") {
            let mut parts = rest.split_whitespace();
            if parts
                .next()
                .is_some_and(|count| count.parse::<u64>().is_ok())
                && parts
                    .next()
                    .is_some_and(|label| label == "test" || label == "tests")
            {
                // `running N tests` includes ignored items. It is useful only
                // as a harness-detection signal, never as executed-count proof.
                self.tests_detected = true;
            }
        }

        if !line.contains("test result:") {
            return;
        }
        self.tests_detected = true;
        match parse_complete_cargo_test_summary_counts(line) {
            Some((passed, failed)) => {
                self.complete_summary_found = true;
                self.tests_passed = self.tests_passed.saturating_add(passed);
                self.tests_failed = self.tests_failed.saturating_add(failed);
                self.tests_run_count = self
                    .tests_run_count
                    .saturating_add(passed)
                    .saturating_add(failed);
            }
            None => {
                // A partial/malformed summary makes the aggregate unproven;
                // do not promote counts observed in other retained sections.
                self.incomplete_summary_found = true;
            }
        }
    }

    fn finish(self) -> CargoTestRunMetadata {
        let status = if self.incomplete_stream {
            CargoTestCountEvidenceStatus::IncompleteStream
        } else if self.incomplete_summary_found {
            CargoTestCountEvidenceStatus::PartialHarnessSummary
        } else if self.complete_summary_found {
            CargoTestCountEvidenceStatus::CompleteSummary
        } else {
            CargoTestCountEvidenceStatus::NoCompleteSummary
        };
        let proven = status.count_is_proven();
        CargoTestRunMetadata {
            tests_detected: self.tests_detected,
            tests_run_count: proven.then_some(self.tests_run_count),
            tests_passed: proven.then_some(self.tests_passed),
            tests_failed: proven.then_some(self.tests_failed),
            zero_tests_run: proven.then_some(self.tests_run_count == 0),
            count_evidence_status: status,
        }
    }
}

#[derive(Debug, Default)]
struct CargoTestStreamBuffer {
    pending: String,
    discarding_oversized_line: bool,
    discarded_saw_test_result: bool,
    marker_tail: Vec<u8>,
}

/// Incremental, bounded Cargo test-count parser for complete process streams.
/// stdout and stderr keep independent line framing so arbitrary chunk boundaries
/// cannot splice two streams or count a summary twice.
#[derive(Debug, Default)]
pub struct CargoTestRunMetadataAccumulator {
    aggregate: CargoTestAggregate,
    stdout: CargoTestStreamBuffer,
    stderr: CargoTestStreamBuffer,
}

impl CargoTestRunMetadataAccumulator {
    pub fn push_stdout_chunk(&mut self, chunk: &str) {
        Self::push_chunk(&mut self.aggregate, &mut self.stdout, chunk);
    }

    pub fn push_stderr_chunk(&mut self, chunk: &str) {
        Self::push_chunk(&mut self.aggregate, &mut self.stderr, chunk);
    }

    pub fn finish(mut self) -> CargoTestRunMetadata {
        Self::finish_stream(&mut self.aggregate, &mut self.stdout);
        Self::finish_stream(&mut self.aggregate, &mut self.stderr);
        self.aggregate.finish()
    }

    fn push_chunk(
        aggregate: &mut CargoTestAggregate,
        stream: &mut CargoTestStreamBuffer,
        mut chunk: &str,
    ) {
        while !chunk.is_empty() {
            if let Some(newline) = chunk.find('\n') {
                let fragment = &chunk[..newline];
                if stream.discarding_oversized_line {
                    Self::scan_discarded_fragment(stream, fragment);
                    Self::finish_discarded_line(aggregate, stream);
                } else {
                    if stream.pending.len().saturating_add(fragment.len())
                        <= CARGO_TEST_STREAM_LINE_MAX_BYTES
                    {
                        stream.pending.push_str(fragment);
                        aggregate.observe_line(&stream.pending);
                    } else {
                        let pending = std::mem::take(&mut stream.pending);
                        stream.discarding_oversized_line = true;
                        Self::scan_discarded_fragment(stream, &pending);
                        Self::scan_discarded_fragment(stream, fragment);
                        Self::finish_discarded_line(aggregate, stream);
                    }
                }
                stream.pending.clear();
                chunk = &chunk[newline + 1..];
            } else {
                if stream.discarding_oversized_line {
                    Self::scan_discarded_fragment(stream, chunk);
                } else {
                    if stream.pending.len().saturating_add(chunk.len())
                        <= CARGO_TEST_STREAM_LINE_MAX_BYTES
                    {
                        stream.pending.push_str(chunk);
                    } else {
                        let pending = std::mem::take(&mut stream.pending);
                        stream.discarding_oversized_line = true;
                        Self::scan_discarded_fragment(stream, &pending);
                        Self::scan_discarded_fragment(stream, chunk);
                    }
                }
                break;
            }
        }
    }

    fn scan_discarded_fragment(stream: &mut CargoTestStreamBuffer, fragment: &str) {
        let marker = CARGO_TEST_RESULT_MARKER;
        let bytes = fragment.as_bytes();
        if !stream.discarded_saw_test_result {
            let direct = bytes.windows(marker.len()).any(|window| window == marker);
            let prefix_len = bytes.len().min(marker.len().saturating_sub(1));
            let mut boundary = Vec::with_capacity(stream.marker_tail.len() + prefix_len);
            boundary.extend_from_slice(&stream.marker_tail);
            boundary.extend_from_slice(&bytes[..prefix_len]);
            let crossed = boundary
                .windows(marker.len())
                .any(|window| window == marker);
            stream.discarded_saw_test_result = direct || crossed;
        }

        let keep = marker.len().saturating_sub(1);
        if keep == 0 {
            stream.marker_tail.clear();
        } else if bytes.len() >= keep {
            stream.marker_tail.clear();
            stream
                .marker_tail
                .extend_from_slice(&bytes[bytes.len() - keep..]);
        } else {
            stream.marker_tail.extend_from_slice(bytes);
            if stream.marker_tail.len() > keep {
                let drop = stream.marker_tail.len() - keep;
                stream.marker_tail.drain(..drop);
            }
        }
    }

    fn finish_discarded_line(
        aggregate: &mut CargoTestAggregate,
        stream: &mut CargoTestStreamBuffer,
    ) {
        if stream.discarded_saw_test_result {
            aggregate.tests_detected = true;
            aggregate.incomplete_stream = true;
        }
        stream.discarding_oversized_line = false;
        stream.discarded_saw_test_result = false;
        stream.marker_tail.clear();
    }

    fn finish_stream(aggregate: &mut CargoTestAggregate, stream: &mut CargoTestStreamBuffer) {
        if stream.discarding_oversized_line {
            Self::finish_discarded_line(aggregate, stream);
        } else if !stream.pending.is_empty() {
            aggregate.observe_line(&stream.pending);
        }
        stream.pending.clear();
    }
}

pub fn parse_cargo_test_run_metadata(text: &str) -> CargoTestRunMetadata {
    let mut accumulator = CargoTestRunMetadataAccumulator::default();
    accumulator.push_stdout_chunk(text);
    accumulator.finish()
}

#[cfg(test)]
mod tests {
    use super::{
        parse_cargo_test_run_metadata, CargoTestRunMetadataAccumulator,
        CARGO_TEST_STREAM_LINE_MAX_BYTES,
    };
    use crate::validation_evidence::CargoTestCountEvidenceStatus;

    #[test]
    fn cargo_test_counts_aggregate_multiple_harness_summaries() {
        let metadata = parse_cargo_test_run_metadata(
            "test result: ok. 2 passed; 0 failed; 1 ignored\n\
             test result: FAILED. 3 passed; 1 failed; 0 ignored\n\
             test result: ok. 0 passed; 0 failed; 2 ignored\n",
        );
        assert_eq!(metadata.tests_run_count, Some(6));
        assert_eq!(metadata.tests_passed, Some(5));
        assert_eq!(metadata.tests_failed, Some(1));
        assert_eq!(
            metadata.count_evidence_status,
            CargoTestCountEvidenceStatus::CompleteSummary
        );
    }

    #[test]
    fn cargo_test_counts_do_not_use_last_summary_wins() {
        let metadata = parse_cargo_test_run_metadata(
            "test result: FAILED. 10 passed; 4 failed; 0 ignored; 0 measured; 0 filtered out\n\
             test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n",
        );
        assert_eq!(metadata.tests_passed, Some(11));
        assert_eq!(metadata.tests_failed, Some(4));
        assert_eq!(metadata.tests_run_count, Some(15));
    }

    #[test]
    fn streaming_parser_handles_chunk_boundaries_and_separate_streams() {
        let mut accumulator = CargoTestRunMetadataAccumulator::default();
        accumulator.push_stdout_chunk("running 2 te");
        accumulator.push_stdout_chunk("sts\ntest result: ok. 1 passed; 0 fai");
        accumulator.push_stdout_chunk("led; 1 ignored\n");
        accumulator.push_stderr_chunk("test result: FAILED. 2 passed; 1 failed; 0 ignored\r\n");
        let metadata = accumulator.finish();
        assert_eq!(metadata.tests_run_count, Some(4));
        assert_eq!(metadata.tests_passed, Some(3));
        assert_eq!(metadata.tests_failed, Some(1));
    }

    #[test]
    fn streaming_parser_fails_closed_on_unbounded_partial_line() {
        let mut accumulator = CargoTestRunMetadataAccumulator::default();
        accumulator.push_stdout_chunk(&format!(
            "{}test result:",
            "x".repeat(CARGO_TEST_STREAM_LINE_MAX_BYTES + 1)
        ));
        accumulator.push_stdout_chunk("\ntest result: ok. 4 passed; 0 failed; 0 ignored\n");
        let metadata = accumulator.finish();
        assert_eq!(metadata.tests_run_count, None);
        assert_eq!(
            metadata.count_evidence_status,
            CargoTestCountEvidenceStatus::IncompleteStream
        );
    }

    #[test]
    fn streaming_parser_ignores_oversized_unrelated_output_lines() {
        let mut accumulator = CargoTestRunMetadataAccumulator::default();
        accumulator.push_stdout_chunk(&"x".repeat(CARGO_TEST_STREAM_LINE_MAX_BYTES + 1));
        accumulator.push_stdout_chunk("\ntest result: ok. 4 passed; 0 failed; 0 ignored\n");
        let metadata = accumulator.finish();
        assert_eq!(metadata.tests_run_count, Some(4));
        assert_eq!(
            metadata.count_evidence_status,
            CargoTestCountEvidenceStatus::CompleteSummary
        );
    }
}
