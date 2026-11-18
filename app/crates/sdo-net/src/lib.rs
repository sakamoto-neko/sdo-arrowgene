//! Asset-independent rooms, synchronized starts and server-side input replay.
mod client;

mod codec;
mod transport;
use anyhow::{Result, ensure};
pub use client::{Client, ClientEvent, Clock};
use serde::{Deserialize, Serialize};
pub use transport::{ServerConnection, ServerListener};

pub use sdo_gameplay::input::SongTime;
mod time_wire {
    use super::SongTime;
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
    pub fn serialize<S: Serializer>(at: &SongTime, out: S) -> Result<S::Ok, S::Error> {
        at.seconds().to_be_bytes().serialize(out)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(input: D) -> Result<SongTime, D::Error> {
        let bytes = <[u8; 8]>::deserialize(input)?;
        SongTime::new(f64::from_be_bytes(bytes))
            .ok_or_else(|| D::Error::custom("Non-finite input time"))
    }
}
pub const PROTOCOL: u32 = 11;
pub const ROOM_SLOTS: usize = 8;
pub const MAX_ROOM_PASSWORD_BYTES: usize = 128;
pub const ROOM_WALK_MIN: [f32; 2] = [0.06, 0.12];
pub const ROOM_WALK_MAX: [f32; 2] = [0.94, 0.94];
/// Normalized room coordinates, shared by the referee and local movement prediction.
pub fn walk_position(position: [f32; 2], direction: [i8; 2], seconds: f32) -> [f32; 2] {
    let [x, y] = direction.map(f32::from);
    let distance = 0.28 * seconds / x.hypot(y).max(1.);
    [
        (position[0] + x * distance).clamp(ROOM_WALK_MIN[0], ROOM_WALK_MAX[0]),
        (position[1] + y * distance).clamp(ROOM_WALK_MIN[1], ROOM_WALK_MAX[1]),
    ]
}
pub const MAX_CHAT_MESSAGES: usize = 100;
pub const MAX_CHAT_BYTES: usize = 400;
pub const DEFAULT_ADDRESS: &str = "127.0.0.1:32100";
pub const MAX_CHART_BYTES: usize = 2 * 1024 * 1024;
pub const LATE_US: u64 = 1_500_000;
pub type PlayerId = u64;
pub type RoomId = u64;
/// Changed chart indices and their authoritative grade, including retractions.
pub type NoteGradeChanges = Vec<(u32, Option<u8>)>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub name: String,
    pub address: String,
    pub listen: String,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            name: "Player".into(),
            address: DEFAULT_ADDRESS.into(),
            listen: "0.0.0.0:32100".into(),
        }
    }
}
/// No local paths or catalog IDs: identity is the actual file, plus its interpretation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChartSpec {
    pub pattern_sha256: String,
    pub audio_sha256: String,
    #[serde(default)]
    pub dance_sha256: Option<String>,
    pub title: String,
    pub kind: String,
    pub difficulty: u8,
    pub judgment: String,
    pub bpm: f64,
    pub duration_us: u64,
}
impl ChartSpec {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            valid_hash(&self.pattern_sha256) && valid_hash(&self.audio_sha256),
            "Invalid SHA-256 identity"
        );
        ensure!(
            self.dance_sha256.as_deref().is_none_or(valid_hash),
            "Invalid choreography identity"
        );
        ensure!(
            self.title.len() <= 200
                && ["k", "t"].contains(&self.kind.as_str())
                && self.difficulty < 3,
            "Invalid chart selection"
        );
        ensure!(
            self.bpm.is_finite() && (10.0..=1000.0).contains(&self.bpm),
            "Invalid chart BPM"
        );
        ensure!(
            (1_000_000..=1_800_000_000).contains(&self.duration_us),
            "Online tracks must last 1 second to 30 minutes"
        );
        ensure!(
            sdo_gameplay::judgment::find(&self.judgment).is_some(),
            "Unknown timing preset"
        );
        Ok(())
    }
}
pub fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Member {
    #[serde(default)]
    pub state_us: u64,
    pub x: f32,
    pub y: f32,
    pub facing: [i8; 2],
    pub walking: [i8; 2],
    pub slot: u8,
    pub id: PlayerId,
    pub name: String,
    pub ready: bool,
}
/// Opaque asset identities only. No pixels, paths, meshes, textures or motions
/// are transmitted. Receivers resolve and verify their own compatible assets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Appearance {
    pub preset: String,
    pub equipment: std::collections::BTreeMap<String, String>,
    pub sha256: String,
}
impl Appearance {
    pub fn validate(&self) -> Result<()> {
        let identity = |s: &str| {
            !s.is_empty()
                && s.len() <= 160
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-.:".contains(c))
        };
        ensure!(
            identity(&self.preset)
                && valid_hash(&self.sha256)
                && self.equipment.len() <= 32
                && self
                    .equipment
                    .iter()
                    .all(|(k, v)| identity(k) && identity(v)),
            "Invalid character appearance"
        );
        Ok(())
    }
}
/// Public discovery information. Detailed room state is delivered only to members.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomSummary {
    pub id: RoomId,
    pub name: String,
    pub occupants: usize,
    pub playing: bool,
    pub password_protected: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Walker {
    pub id: PlayerId,
    pub position: [f32; 2],
    pub facing: [i8; 2],
    pub walking: [i8; 2],
}
impl Walker {
    pub fn apply(self, member: &mut Member, at: u64) {
        [member.x, member.y] = self.position;
        member.state_us = at;
        member.facing = self.facing;
        member.walking = self.walking;
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Room {
    pub keys: u8,
    pub levels: [Option<u16>; 3],
    pub password_protected: bool,
    pub id: RoomId,
    pub name: String,
    pub host: PlayerId,
    pub members: Vec<Member>,
    pub chart: Option<ChartSpec>,
    pub round: Option<u64>,
    pub start_us: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: u64,
    pub player: Option<PlayerId>,
    pub name: String,
    pub text: String,
    pub error: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Standing {
    pub health: i32,
    pub player: PlayerId,
    pub points: u64,
    pub counts: [u32; 4],
    pub combo: u32,
    pub max_combo: u32,
    pub failed: bool,
    pub forfeited: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Command {
    Hello {
        protocol: u32,
        name: String,
    },
    Ping {
        sent_us: u64,
    },
    Create {
        name: String,
        password: String,
    },
    OpenPrivateRoom {
        name: String,
        password: String,
        key: String,
    },
    Join {
        room: RoomId,
        password: String,
    },
    Leave,
    Walk {
        direction: [i8; 2],
    },
    TakeSlot {
        slot: u8,
    },
    SetAppearance {
        appearance: Option<Appearance>,
    },
    Offer {
        spec: ChartSpec,
        raw: Vec<u8>,
    },
    Ready {
        spec: ChartSpec,
        offset_us: i64,
    },
    Unready,
    TransferHost {
        player: PlayerId,
    },
    SetDifficulty {
        difficulty: u8,
    },
    Chat {
        text: String,
    },
    Unavailable {
        spec: ChartSpec,
        reason: String,
    },
    Start,
    Input {
        round: u64,
        sequence: u64,
        #[serde(with = "time_wire")]
        song_time: SongTime,
        lane: u8,
        down: bool,
    },
    Forfeit {
        round: u64,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Event {
    Welcome {
        protocol: u32,
        player: PlayerId,
        private_server: bool,
    },
    RoomPasswordRequired {
        room: RoomId,
        name: String,
    },
    Pong {
        sent_us: u64,
        server_us: u64,
    },
    Directory(Vec<RoomSummary>),
    RoomState(Option<Box<Room>>),
    Movement {
        room: RoomId,
        state_us: u64,
        walkers: Vec<Walker>,
    },
    Started {
        room: RoomId,
        round: u64,
        start_us: u64,
        offsets: std::collections::BTreeMap<PlayerId, i64>,
    },
    Input {
        room: RoomId,
        round: u64,
        player: PlayerId,
        sequence: u64,
        #[serde(with = "time_wire")]
        song_time: SongTime,
        lane: u8,
        down: bool,
    },
    Judgments {
        room: RoomId,
        round: u64,
        revision: u64,
        /// Stable chart index and grade, or None when rollback retracts a judgment.
        changes: NoteGradeChanges,
    },
    Scores {
        room: RoomId,
        round: u64,
        standings: Vec<Standing>,
        final_result: bool,
        server_us: u64,
    },
    Aborted {
        room: RoomId,
        round: u64,
        reason: String,
    },
    Chat {
        room: RoomId,
        message: ChatMessage,
    },
    ChatHistory {
        room: RoomId,
        messages: Vec<ChatMessage>,
    },
    Appearance {
        room: RoomId,
        player: PlayerId,
        appearance: Option<Appearance>,
    },
    Error(String),
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    #[test]
    fn protocol_sends_small_state_packets_and_rejects_image_messages() {
        let appearance = Appearance {
            preset: "male".into(),
            equipment: std::collections::BTreeMap::from([("hair".into(), "001234".into())]),
            sha256: "a".repeat(64),
        };
        appearance.validate().unwrap();
        let packet = serde_json::to_vec(&Command::SetAppearance {
            appearance: Some(appearance),
        })
        .unwrap();
        assert!(packet.len() < 512);
        assert!(!String::from_utf8(packet).unwrap().contains("rgba"));
        let packet = serde_json::to_vec(&Event::Input {
            room: 1,
            round: 2,
            player: 3,
            sequence: 4,
            song_time: SongTime::new(1.5).unwrap(),
            lane: 0,
            down: true,
        })
        .unwrap();
        assert!(packet.len() < 160);
        assert!(
            serde_json::from_str::<Command>(
                r#"{"type":"SetPortrait","data":{"portrait":{"rgba":[255],"avatar":[255]}}}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<Event>(
                r#"{"type":"Portrait","data":{"room":1,"player":1,"portrait":null}}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<Appearance>(
                r#"{"preset":"male","equipment":{},"sha256":"bad","rgba":[255]}"#
            )
            .is_err()
        );
    }
}
