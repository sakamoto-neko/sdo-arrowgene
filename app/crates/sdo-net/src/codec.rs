//! Control messages use tagged JSON. Timing edges and chart bytes have fixed binary framing.
use crate::{Command, Event, MAX_CHART_BYTES, SongTime};
use anyhow::{Context, Result, ensure};
use serde::{Serialize, de::DeserializeOwned};
pub const MAX_FRAME: usize = 3 * 1024 * 1024;
const INPUT: u8 = 1;
const OFFER: u8 = 2;

pub trait Packet: Serialize + DeserializeOwned {
    fn encode(&self) -> Result<Vec<u8>>;
    fn decode(bytes: &[u8]) -> Result<Self>;
}
fn edge(ids: &[u64], time: SongTime, lane: u8, down: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(11 + ids.len() * 8);
    bytes.push(INPUT);
    for id in ids {
        bytes.extend(id.to_be_bytes())
    }
    bytes.extend(time.seconds().to_be_bytes());
    bytes.extend([lane, u8::from(down)]);
    bytes
}
fn read_edge<const N: usize>(bytes: &[u8]) -> Result<([u64; N], SongTime, u8, bool)> {
    ensure!(bytes.len() == 11 + 8 * N, "Invalid input packet length");
    let (fields, tail) = bytes[1..].as_chunks::<8>();
    let ids = std::array::from_fn(|i| u64::from_be_bytes(fields[i]));
    let at = SongTime::new(f64::from_be_bytes(fields[N])).context("Invalid input timestamp")?;
    ensure!(tail[0] < 4 && tail[1] <= 1, "Invalid lane transition");
    Ok((ids, at, tail[0], tail[1] == 1))
}
impl Packet for Command {
    fn encode(&self) -> Result<Vec<u8>> {
        match self {
            Self::Input {
                round,
                sequence,
                song_time,
                lane,
                down,
            } => Ok(edge(&[*round, *sequence], *song_time, *lane, *down)),
            Self::Offer { spec, raw } => {
                ensure!(
                    raw.len() <= MAX_CHART_BYTES,
                    "Chart exceeds online size limit"
                );
                let metadata = serde_json::to_vec(spec)?;
                let size = u16::try_from(metadata.len())?;
                let mut bytes = Vec::with_capacity(3 + metadata.len() + raw.len());
                bytes.push(OFFER);
                bytes.extend(size.to_be_bytes());
                bytes.extend(metadata);
                bytes.extend(raw);
                Ok(bytes)
            }
            _ => Ok(serde_json::to_vec(self)?),
        }
    }
    fn decode(bytes: &[u8]) -> Result<Self> {
        match bytes.first() {
            Some(&INPUT) => {
                let ([round, sequence], song_time, lane, down) = read_edge(bytes)?;
                Ok(Self::Input {
                    round,
                    sequence,
                    song_time,
                    lane,
                    down,
                })
            }
            Some(&OFFER) => {
                ensure!(bytes.len() >= 3, "Incomplete chart offer");
                let end = 3 + u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
                let raw = bytes.get(end..).context("Invalid offer metadata length")?;
                ensure!(
                    raw.len() <= MAX_CHART_BYTES,
                    "Chart exceeds online size limit"
                );
                Ok(Self::Offer {
                    spec: serde_json::from_slice(&bytes[3..end])?,
                    raw: raw.to_vec(),
                })
            }
            _ => Ok(serde_json::from_slice(bytes)?),
        }
    }
}
impl Packet for Event {
    fn encode(&self) -> Result<Vec<u8>> {
        if let Self::Input {
            room,
            round,
            player,
            sequence,
            song_time,
            lane,
            down,
        } = self
        {
            Ok(edge(
                &[*room, *round, *player, *sequence],
                *song_time,
                *lane,
                *down,
            ))
        } else {
            Ok(serde_json::to_vec(self)?)
        }
    }
    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.first() == Some(&INPUT) {
            let ([room, round, player, sequence], song_time, lane, down) = read_edge(bytes)?;
            Ok(Self::Input {
                room,
                round,
                player,
                sequence,
                song_time,
                lane,
                down,
            })
        } else {
            Ok(serde_json::from_slice(bytes)?)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_edges_survive_both_wire_directions() -> Result<()> {
        for raw in [
            -0.5,
            -0.,
            0.,
            1.0624998,
            1.0625_f64.next_down(),
            1.0625,
            1.0625_f64.next_up(),
            1799.9999999999,
        ] {
            let time = SongTime::new(raw).unwrap();
            let bytes = Command::Input {
                round: 4,
                sequence: 100,
                song_time: time,
                lane: 3,
                down: false,
            }
            .encode()?;
            assert_eq!(bytes.len(), 27);
            let Command::Input {
                round,
                sequence,
                song_time,
                lane,
                down,
            } = Command::decode(&bytes)?
            else {
                panic!()
            };
            let bytes = Event::Input {
                room: 1,
                player: 2,
                round,
                sequence,
                song_time,
                lane,
                down,
            }
            .encode()?;
            let Event::Input { song_time, .. } = Event::decode(&bytes)? else {
                panic!()
            };
            assert_eq!(song_time.seconds().to_bits(), raw.to_bits());
        }
        let mut invalid = vec![0; 27];
        invalid[0] = INPUT;
        invalid[17..25].copy_from_slice(&f64::NAN.to_be_bytes());
        assert!(Command::decode(&invalid).is_err());
        Ok(())
    }
}
