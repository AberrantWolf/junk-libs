//! Headless playback identities and integer frame coordinates.

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Generation(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueTrack<K> {
    pub key: K,
    pub total_frames: u64,
}

impl<K> QueueTrack<K> {
    #[must_use]
    pub fn new(key: K, total_frames: u64) -> Self {
        Self { key, total_frames }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackPosition<K> {
    pub track_key: K,
    pub frame: u64,
    pub generation: Generation,
}

impl<K: Clone> TrackPosition<K> {
    pub fn new(
        track: &QueueTrack<K>,
        frame: u64,
        generation: Generation,
    ) -> Result<Self, PlaybackContractError> {
        if frame > track.total_frames {
            return Err(PlaybackContractError::FrameOutOfRange {
                frame,
                total_frames: track.total_frames,
            });
        }
        Ok(Self {
            track_key: track.key.clone(),
            frame,
            generation,
        })
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackContractError {
    #[error("frame {frame} exceeds track length {total_frames}")]
    FrameOutOfRange { frame: u64, total_frames: u64 },
}
