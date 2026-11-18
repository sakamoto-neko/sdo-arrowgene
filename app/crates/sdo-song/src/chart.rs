//! Checked, read-only parsing of classic GN 2.9 four-key charts.
//! Unknown header bytes, events, flags and trailing sections remain available.
use anyhow::{Context, Result, ensure};
use sdo_gameplay::{Lane, Note, TempoChange};
use std::{fs, io::Read, path::Path};

#[derive(Debug, Clone, Copy)]
pub enum Difficulty {
    Easy,
    Normal,
    Hard,
}
impl Difficulty {
    pub const ALL: [Self; 3] = [Self::Easy, Self::Normal, Self::Hard];
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "easy" => Ok(Self::Easy),
            "normal" => Ok(Self::Normal),
            "hard" => Ok(Self::Hard),
            _ => anyhow::bail!("Difficulty must be easy, normal or hard"),
        }
    }
    pub fn index(self) -> usize {
        match self {
            Self::Easy => 0,
            Self::Normal => 1,
            Self::Hard => 2,
        }
    }
}
/// Shared GN metadata for the song browser and full chart parser. Reading the
/// fixed header avoids decoding notes or audio just to display difficulty levels.
pub struct ChartHeader {
    pub levels: [u16; 3],
    offsets: [usize; 4],
}
impl ChartHeader {
    pub fn load(path: &Path) -> Result<Self> {
        let read = || -> Result<Self> {
            let mut file = fs::File::open(path)?;
            let mut raw = [0; 300];
            file.read_exact(&mut raw).context("Truncated GN header")?;
            let header = Self::parse(&raw)?;
            header.validate_length(file.metadata()?.len())?;
            Ok(header)
        };
        read().with_context(|| format!("Cannot read GN chart header {}", path.display()))
    }
    pub fn parse(raw: &[u8]) -> Result<Self> {
        ensure!(raw.len() >= 300, "Truncated GN header");
        ensure!(&raw[4..8] == b"gn\0\0", "Expected a decrypted GN chart");
        let version = f32::from_bits(u32_at(raw, 8));
        ensure!(
            (version - 2.9).abs() < 0.001,
            "Unsupported GN version {version}"
        );
        let offsets = std::array::from_fn(|i| u32_at(raw, 284 + i * 4) as usize);
        ensure!(
            offsets[0] >= 300 && offsets.windows(2).all(|v| v[0] <= v[1]),
            "Invalid GN difficulty offsets"
        );
        Ok(Self {
            levels: std::array::from_fn(|i| u16_at(raw, 20 + i * 2)),
            offsets,
        })
    }
    pub(crate) fn validate_length(&self, length: u64) -> Result<()> {
        ensure!(
            self.offsets[3] as u64 <= length,
            "Invalid GN difficulty offsets"
        );
        Ok(())
    }
    pub fn level(&self, difficulty: Difficulty) -> Option<u16> {
        let index = difficulty.index();
        (self.offsets[index] < self.offsets[index + 1]).then_some(self.levels[index])
    }
}
#[derive(Debug, Clone)]
pub struct Event {
    pub measure: u32,
    pub kind: u16,
    pub subdivisions: u16,
    pub values: Vec<u32>,
}
pub struct Chart {
    pub bpm: f64,
    pub header_bpm: f64,
    pub title: String,
    pub level: u16,
    pub notes: Vec<Note>,
    pub tempo: Vec<TempoChange>,
    pub raw_note_events: usize,
    pub note_count_warning: Option<String>,
    pub events: Vec<Event>,
    pub raw: Vec<u8>,
    pub header_field_12: u32,
    pub trailing_bytes: usize,
    pub additional_lane_events: usize,
}
fn u16_at(raw: &[u8], i: usize) -> u16 {
    u16::from_le_bytes(raw[i..i + 2].try_into().unwrap())
}
fn u32_at(raw: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(raw[i..i + 4].try_into().unwrap())
}
impl Chart {
    pub fn into_practice(
        self,
        duration: f64,
        preset: sdo_gameplay::Preset,
        difficulty: Difficulty,
    ) -> sdo_gameplay::Practice {
        sdo_gameplay::Practice::from_notes(self.notes, duration, preset)
            .with_bpm(self.bpm)
            .with_tempo(self.tempo)
            .with_health_difficulty(sdo_gameplay::HealthDifficulty::from_index(
                difficulty.index(),
            ))
    }
    pub fn load(path: &Path, difficulty: Difficulty) -> Result<Self> {
        Self::parse(
            fs::read(path).with_context(|| format!("Cannot open {}", path.display()))?,
            difficulty,
        )
        .with_context(|| format!("Invalid GN chart {}", path.display()))
    }
    /// Use the catalog pattern BPM (or inherited song BPM) without rewriting GN data.
    pub fn load_with_bpm(path: &Path, difficulty: Difficulty, bpm: f64) -> Result<Self> {
        Self::parse_with_bpm(
            fs::read(path).with_context(|| format!("Cannot open {}", path.display()))?,
            difficulty,
            Some(bpm),
        )
        .with_context(|| format!("Invalid GN chart {}", path.display()))
    }
    pub fn parse(raw: Vec<u8>, difficulty: Difficulty) -> Result<Self> {
        Self::parse_with_bpm(raw, difficulty, None)
    }
    pub fn parse_with_bpm(
        raw: Vec<u8>,
        difficulty: Difficulty,
        bpm_override: Option<f64>,
    ) -> Result<Self> {
        let header = ChartHeader::parse(&raw)?;
        header.validate_length(raw.len() as u64)?;
        let header_bpm = f32::from_bits(u32_at(&raw, 16)) as f64;
        // Original GN timing uses float32 BPM; keep compact JSON decimals on that scale.
        let bpm = bpm_override
            .map(|value| value as f32 as f64)
            .unwrap_or(header_bpm);
        ensure!(
            bpm.is_finite() && (10.0..=1000.0).contains(&bpm),
            "Invalid GN BPM"
        );
        let addresses = header.offsets;
        let index = difficulty.index();
        let mut cursor = addresses[index];
        let limit = addresses[index + 1];
        let mut events = Vec::new();
        let mut markers = Vec::new();
        let mut additional_lane_events = 0;
        let mut raw_note_events = 0;
        let mut tempo_events = Vec::new();
        while cursor < limit {
            ensure!(limit - cursor >= 8, "Truncated GN event header");
            let measure = u32_at(&raw, cursor);
            let kind = u16_at(&raw, cursor + 4);
            let subdivisions = u16_at(&raw, cursor + 6);
            cursor += 8;
            ensure!(
                subdivisions > 0 && subdivisions <= 4096,
                "Invalid GN subdivisions"
            );
            let payload = subdivisions as usize * 4;
            ensure!(payload <= limit - cursor, "Truncated GN event values");
            let values = (0..subdivisions as usize)
                .map(|i| u32_at(&raw, cursor + i * 4))
                .collect::<Vec<_>>();
            if kind == 1 {
                for (slot, &value) in values.iter().enumerate() {
                    if value == 0 {
                        continue;
                    }
                    let tempo = f32::from_bits(value) as f64;
                    ensure!(
                        tempo.is_finite() && (10.0..=1000.0).contains(&tempo),
                        "Invalid GN tempo {tempo} at measure {measure}"
                    );
                    tempo_events.push((
                        (measure as f64 + slot as f64 / subdivisions as f64) * 4.0,
                        tempo,
                    ));
                }
            }
            if (6..=8).contains(&kind) {
                additional_lane_events += values.iter().filter(|&&v| v != 0).count();
            }
            if (2..=5).contains(&kind) {
                for (slot, &value) in values.iter().enumerate() {
                    if value == 0 {
                        continue;
                    }
                    // The low 24 bits are note/keysound payload, not hold markers.
                    // The native four-key renderer masks 0x03000000 for note class.
                    let marker = match value & 0x03000000 {
                        0x02000000 => 2,
                        0x03000000 => 3,
                        _ => 1,
                    };
                    raw_note_events += 1;
                    let beat = (measure as f64 + slot as f64 / subdivisions as f64) * 4.0;
                    markers.push((beat, Lane::ALL[(kind - 2) as usize], marker));
                }
            }
            events.push(Event {
                measure,
                kind,
                subdivisions,
                values,
            });
            cursor += payload;
        }
        tempo_events.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut tempo = vec![TempoChange {
            beat: 0.0,
            at: 0.0,
            bpm,
        }];
        for (beat, next_bpm) in tempo_events {
            let last = *tempo.last().unwrap();
            let at = last.at + (beat - last.beat) * 60.0 / last.bpm;
            if beat == last.beat {
                tempo.last_mut().unwrap().bpm = next_bpm;
            } else if next_bpm != last.bpm {
                tempo.push(TempoChange {
                    beat,
                    at,
                    bpm: next_bpm,
                });
            }
        }
        let seconds_at = |beat: f64| {
            let index = tempo.partition_point(|t| t.beat <= beat).saturating_sub(1);
            let t = tempo[index];
            t.at + (beat - t.beat) * 60.0 / t.bpm
        };
        markers.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut holds: [Option<usize>; 4] = [None; 4];
        let mut notes: Vec<Note> = Vec::new();
        for (beat, lane, marker) in markers {
            let time = seconds_at(beat);
            let hold = &mut holds[lane.index()];
            match marker {
                3 => {
                    // Releases are not playable onsets (USA client.bin, 0x53f12a).
                    if let Some(index) = hold.take() {
                        let note = &mut notes[index];
                        ensure!(time > note.start, "GN hold has zero/negative length");
                        note.end = time;
                    }
                }
                1 | 2 => {
                    if marker == 2 {
                        ensure!(hold.is_none(), "Overlapping GN hold starts");
                        *hold = Some(notes.len());
                    }
                    // The native length scan returns zero when it finds no release
                    // (USA 0x53acb6, standalone 2006 0x495d30). Keep the authored onset.
                    notes.push(Note {
                        lane,
                        start: time,
                        end: time,
                    });
                }
                _ => unreachable!(),
            }
        }
        notes.sort_by(|a, b| {
            a.start
                .total_cmp(&b.start)
                .then(a.lane.index().cmp(&b.lane.index()))
        });
        ensure!(
            !notes.is_empty(),
            "No four-key notes in selected difficulty"
        );
        let expected = u32_at(&raw, 40 + 4 * index) as usize;
        let note_count_warning = (expected != 0
            && expected != raw_note_events
            && expected != raw_note_events + additional_lane_events)
            .then(|| format!(
                "GN header declares {expected} note events, decoded {raw_note_events} four-key events plus {additional_lane_events} additional lane events"
            ));
        let title = String::from_utf8_lossy(&raw[108..140])
            .trim_end_matches('\0')
            .to_owned();
        let level = header.levels[index];
        let header_field_12 = u32_at(&raw, 12);
        let trailing_bytes = raw.len() - addresses[3];
        Ok(Self {
            bpm,
            header_bpm,
            title,
            level,
            notes,
            tempo,
            raw_note_events,
            note_count_warning,
            events,
            raw,
            header_field_12,
            trailing_bytes,
            additional_lane_events,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Vec<u8> {
        let mut d = vec![0; 300];
        d[4..8].copy_from_slice(b"gn\0\0");
        d[8..12].copy_from_slice(&2.9f32.to_le_bytes());
        d[16..20].copy_from_slice(&120f32.to_le_bytes());
        d[40..44].copy_from_slice(&3u32.to_le_bytes());
        for (measure, kind, values) in [
            (0u32, 2u16, vec![1u32, 0, 0x02000001, 0]),
            (1, 2, vec![0, 0x03000001, 0, 0]),
        ] {
            d.extend(measure.to_le_bytes());
            d.extend(kind.to_le_bytes());
            d.extend((values.len() as u16).to_le_bytes());
            for value in values {
                d.extend(value.to_le_bytes());
            }
        }
        let end = d.len() as u32;
        for (i, v) in [300, end, end, end].into_iter().enumerate() {
            d[284 + i * 4..288 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        d
    }
    #[test]
    fn subdivisions_and_cross_measure_holds() {
        let c = Chart::parse(fixture(), Difficulty::Easy).unwrap();
        assert_eq!(c.notes.len(), 2);
        assert_eq!(c.notes[0].start, 0.0);
        assert_eq!(c.notes[1].start, 1.0);
        assert_eq!(c.notes[1].end, 2.5);
        assert!(c.note_count_warning.is_none());
    }
    #[test]
    fn header_count_mismatch_warns_and_uses_authored_notes() {
        for declared in [2u32, 4] {
            let mut raw = fixture();
            raw[40..44].copy_from_slice(&declared.to_le_bytes());
            let chart = Chart::parse(raw.clone(), Difficulty::Easy).unwrap();
            assert!(chart.note_count_warning.is_some());
            assert_eq!(chart.raw, raw);
            assert_eq!(chart.raw_note_events, 3);
            assert_eq!(chart.notes.len(), 2);
            assert_eq!((chart.notes[0].start, chart.notes[0].end), (0.0, 0.0));
            assert_eq!((chart.notes[1].start, chart.notes[1].end), (1.0, 2.5));
        }
    }
    #[test]
    fn browser_levels_match_full_parsing_and_empty_difficulties_stay_unavailable() -> Result<()> {
        let mut raw = fixture();
        let events = raw[300..].to_vec();
        raw.extend(&events);
        raw.extend(&events);
        for (i, level) in [3u16, 8, 32].into_iter().enumerate() {
            raw[20 + i * 2..22 + i * 2].copy_from_slice(&level.to_le_bytes());
            raw[40 + i * 4..44 + i * 4].copy_from_slice(&3u32.to_le_bytes());
        }
        for i in 0..4 {
            raw[284 + i * 4..288 + i * 4]
                .copy_from_slice(&((300 + i * events.len()) as u32).to_le_bytes());
        }
        let path = std::env::temp_dir().join(format!("sdo-chart-levels-{}.gn", std::process::id()));
        fs::write(&path, &raw)?;
        let header = ChartHeader::load(&path)?;
        for difficulty in Difficulty::ALL {
            assert_eq!(
                header.level(difficulty),
                Some(Chart::parse(raw.clone(), difficulty)?.level)
            );
        }
        let mut empty = fixture();
        empty[22..24].copy_from_slice(&9u16.to_le_bytes());
        assert_eq!(ChartHeader::parse(&empty)?.level(Difficulty::Normal), None);
        assert_eq!(
            ChartHeader::parse(&empty)?.level(Difficulty::Easy),
            Some(0),
            "Keep authored zero levels, without inventing a rating"
        );
        fs::write(&path, &raw[..300])?;
        assert!(
            ChartHeader::load(&path).is_err(),
            "A header claiming missing event data is invalid"
        );
        for len in 0..300 {
            assert!(ChartHeader::parse(&raw[..len]).is_err());
        }
        raw[8..12].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(ChartHeader::parse(&raw).is_err());
        fs::remove_file(path)?;
        Ok(())
    }
    #[test]
    fn catalog_bpm_override_changes_timing_without_editing_gn_bytes() {
        let raw = fixture();
        let chart = Chart::parse_with_bpm(raw.clone(), Difficulty::Easy, Some(240.0)).unwrap();
        assert_eq!(chart.header_bpm, 120.0);
        assert_eq!(chart.bpm, 240.0);
        assert_eq!(chart.notes[1].start, 0.5);
        assert_eq!(chart.notes[1].end, 1.25);
        assert_eq!(chart.raw, raw);
        assert!(Chart::parse_with_bpm(fixture(), Difficulty::Easy, Some(0.0)).is_err());
    }
    #[test]
    fn additional_lanes_are_retained_without_mapping_them_to_four_key_notes() {
        let mut raw = fixture();
        raw.extend(0u32.to_le_bytes());
        raw.extend(6u16.to_le_bytes());
        raw.extend(1u16.to_le_bytes());
        raw.extend(0x02000001u32.to_le_bytes());
        let end = raw.len() as u32;
        for offset in [288, 292, 296] {
            raw[offset..offset + 4].copy_from_slice(&end.to_le_bytes());
        }
        raw[40..44].copy_from_slice(&4u32.to_le_bytes());
        let chart = Chart::parse(raw.clone(), Difficulty::Easy).unwrap();
        assert_eq!(chart.notes.len(), 2);
        assert_eq!(chart.additional_lane_events, 1);
        assert!(chart.note_count_warning.is_none());
        assert_eq!(chart.raw, raw);
    }
    #[test]
    fn truncation_is_an_error_not_a_panic() {
        let d = fixture();
        for len in 0..d.len() {
            assert!(Chart::parse(d[..len].to_vec(), Difficulty::Easy).is_err());
        }
    }
    #[test]
    fn extra_release_creates_no_onset_and_does_not_extend_a_completed_hold() {
        let mut d = fixture();
        d[340..344].copy_from_slice(&0x03000001u32.to_le_bytes());
        d[40..44].copy_from_slice(&4u32.to_le_bytes());
        let chart = Chart::parse(d.clone(), Difficulty::Easy).unwrap();
        assert_eq!(chart.raw_note_events, 4);
        assert_eq!(chart.notes.len(), 2);
        assert_eq!((chart.notes[1].start, chart.notes[1].end), (1.0, 2.5));
        assert_eq!(chart.raw, d);
    }
    #[test]
    fn unmatched_head_keeps_its_position_and_release_on_another_lane_cannot_end_it() {
        let mut d = fixture();
        d[328..330].copy_from_slice(&3u16.to_le_bytes());
        let chart = Chart::parse(d, Difficulty::Easy).unwrap();
        assert_eq!(chart.raw_note_events, 3);
        assert_eq!(chart.notes.len(), 2);
        assert_eq!(chart.notes[1].lane, Lane::Left);
        assert_eq!((chart.notes[1].start, chart.notes[1].end), (1.0, 1.0));
    }
    #[test]
    fn note_payload_ids_are_not_hold_markers() {
        let mut d = fixture();
        d[308..312].copy_from_slice(&3u32.to_le_bytes());
        d[316..320].copy_from_slice(&0x02000142u32.to_le_bytes());
        let chart = Chart::parse(d, Difficulty::Easy).unwrap();
        assert!(!chart.notes[0].is_hold());
        assert!(chart.notes[1].is_hold());
        assert_eq!(chart.raw_note_events, 3);
    }
    #[test]
    fn tempo_changes_integrate_time_for_taps_and_holds() {
        let mut d = fixture();
        // At beat 2, double from 120 to 240 BPM, while a hold begins.
        d.extend(0u32.to_le_bytes());
        d.extend(1u16.to_le_bytes());
        d.extend(2u16.to_le_bytes());
        d.extend(0u32.to_le_bytes());
        d.extend(240f32.to_le_bytes());
        let end = d.len() as u32;
        for at in [288, 292, 296] {
            d[at..at + 4].copy_from_slice(&end.to_le_bytes());
        }
        let chart = Chart::parse(d, Difficulty::Easy).unwrap();
        assert_eq!(chart.tempo.len(), 2);
        assert_eq!(chart.tempo[1].at, 1.);
        assert_eq!(chart.notes[1].start, 1.);
        assert_eq!(chart.notes[1].end, 1.75);
    }
}

/// SHA-256 over the complete pattern file bytes, including every difficulty and header.
pub fn pattern_sha256(raw: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(raw))
}
impl Chart {
    pub fn pattern_sha256(&self) -> String {
        pattern_sha256(&self.raw)
    }
}

#[cfg(test)]
mod hash_tests {
    #[test]
    fn sha256_is_over_exact_pattern_bytes() {
        assert_eq!(
            super::pattern_sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            super::pattern_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_ne!(
            super::pattern_sha256(b"abc"),
            super::pattern_sha256(b"abc\0")
        );
    }
}
