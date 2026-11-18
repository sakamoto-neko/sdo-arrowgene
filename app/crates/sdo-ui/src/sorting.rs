use crate::SongSelector;
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SongSort {
    #[default]
    Id,
    Level,
    Name,
    Bpm,
    Time,
}
impl SongSort {
    pub const ALL: [Self; 5] = [Self::Id, Self::Level, Self::Name, Self::Bpm, Self::Time];
    pub fn id(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Level => "level",
            Self::Name => "name",
            Self::Bpm => "bpm",
            Self::Time => "time",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Id => "Song ID",
            Self::Level => "Level",
            Self::Name => "Name",
            Self::Bpm => "BPM",
            Self::Time => "Time",
        }
    }
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|sort| sort.id() == id)
    }
}
fn compare_ids(a: &str, b: &str) -> Ordering {
    let numeric = |id: &str| !id.is_empty() && id.bytes().all(|c| c.is_ascii_digit());
    match (numeric(a), numeric(b)) {
        (false, false) => return a.cmp(b),
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        (true, true) => {}
    }
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}
impl SongSelector {
    pub(super) fn sort_visible(&mut self) {
        let songs = &self.catalog.songs;
        let kind = &self.kind;
        let difficulty = self.difficulty;
        let sort = self.sort;
        let descending = self.sort_descending;
        self.visible.sort_by(|&a, &b| {
            let (a, b) = (&songs[a], &songs[b]);
            let order = match sort {
                SongSort::Id => compare_ids(&a.id, &b.id),
                SongSort::Name => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
                SongSort::Bpm => {
                    let bpm = |song: &sdo_song::catalog::Song| {
                        song.pattern(kind, 4)
                            .ok()
                            .and_then(|p| p.bpm)
                            .unwrap_or(song.bpm)
                    };
                    bpm(a).total_cmp(&bpm(b))
                }
                SongSort::Time => a.duration.total_cmp(&b.duration),
                SongSort::Level => {
                    let level = |song: &sdo_song::catalog::Song| {
                        song.pattern(kind, 4)
                            .ok()
                            .and_then(|p| p.levels[difficulty])
                    };
                    match (level(a), level(b)) {
                        (Some(a), Some(b)) => a.cmp(&b),
                        // Unknown ratings stay last in both directions.
                        (None, Some(_)) => return Ordering::Greater,
                        (Some(_), None) => return Ordering::Less,
                        (None, None) => Ordering::Equal,
                    }
                }
            };
            let order = if descending { order.reverse() } else { order };
            order.then_with(|| compare_ids(&a.id, &b.id))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_ids_preserve_identity_and_sort_after_numeric_ids() {
        let mut ids = ["cn:b", "10", "cn:aa", "2", "0001"];
        ids.sort_by(|a, b| compare_ids(a, b));
        assert_eq!(ids, ["0001", "2", "10", "cn:aa", "cn:b"]);
    }
    fn ids(selector: &SongSelector) -> Vec<&str> {
        selector
            .visible
            .iter()
            .map(|&i| selector.catalog.songs[i].id.as_str())
            .collect()
    }
    #[test]
    fn sorts_use_current_chart_metadata_and_keep_song_identity() {
        let mut s = crate::tests::selector();
        for (song, title, bpm, duration, levels) in [
            (0, "beta", 200., 60., [Some(7), Some(3), Some(0)]),
            (1, "Alpha", 80., 180., [Some(2), Some(9), Some(1)]),
            (2, "gamma", 150., 90., [None; 3]),
        ] {
            s.catalog.songs[song].title = title.into();
            s.catalog.songs[song].bpm = bpm;
            s.catalog.songs[song].duration = duration;
            s.catalog.songs[song].patterns[0].levels = levels;
        }
        s.catalog.songs[0].patterns[0].bpm = Some(100.);
        s.move_by(1);
        for (sort, expected) in [
            (SongSort::Name, ["00002", "00001", "00003"]),
            (SongSort::Bpm, ["00002", "00001", "00003"]),
            (SongSort::Time, ["00001", "00003", "00002"]),
            (SongSort::Level, ["00002", "00001", "00003"]),
        ] {
            s.sort = sort;
            s.refilter();
            assert_eq!(ids(&s), expected, "{sort:?}");
            assert_eq!(s.selected().unwrap().id, "00002");
        }
        s.sort_descending = true;
        s.sort_visible();
        assert_eq!(
            ids(&s),
            ["00001", "00002", "00003"],
            "Unknown levels stay last when descending"
        );
        s.difficulty = 1;
        s.sort_visible();
        assert_eq!(ids(&s), ["00002", "00001", "00003"]);
        s.difficulty = 2;
        s.sort_descending = false;
        s.sort_visible();
        assert_eq!(
            ids(&s),
            ["00001", "00002", "00003"],
            "An authored zero level is known"
        );
        s.query = "beta".into();
        s.refilter();
        assert_eq!(ids(&s), ["00001"]);
        s.query.clear();
        s.source = Some("usa".into());
        s.refilter();
        assert_eq!(ids(&s), ["00003"]);
    }
    #[test]
    fn both_directions_and_ties_have_deterministic_order() {
        assert_eq!(compare_ids("2", "00010"), Ordering::Less);
        let mut s = crate::tests::selector();
        for sort in [SongSort::Id, SongSort::Name, SongSort::Bpm, SongSort::Time] {
            s.sort = sort;
            s.sort_descending = false;
            s.sort_visible();
            let ascending = s.visible.clone();
            s.sort_descending = true;
            s.sort_visible();
            if matches!(sort, SongSort::Id | SongSort::Name) {
                assert_eq!(s.visible, ascending.into_iter().rev().collect::<Vec<_>>());
            } else {
                assert_eq!(s.visible, ascending, "Equal values keep stable ID order");
            }
        }
    }
}
