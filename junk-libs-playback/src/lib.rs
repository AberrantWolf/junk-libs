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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueSnapshot<K> {
    pub tracks: Vec<QueueTrack<K>>,
    pub position: Option<TrackPosition<K>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueTransition<K> {
    pub generation: Generation,
    pub position: Option<TrackPosition<K>>,
}

/// Deterministic, headless playback queue policy.
///
/// A generation changes whenever buffered audio must be discarded. Appending or
/// truncating only later tracks therefore keeps the current generation stable.
#[derive(Debug, Clone)]
pub struct QueueState<K> {
    tracks: Vec<QueueTrack<K>>,
    current: Option<usize>,
    frame: u64,
    generation: Generation,
}

impl<K> Default for QueueState<K> {
    fn default() -> Self {
        Self {
            tracks: Vec::new(),
            current: None,
            frame: 0,
            generation: Generation(0),
        }
    }
}

impl<K: Clone> QueueState<K> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn restore(
        tracks: Vec<QueueTrack<K>>,
        current: Option<usize>,
        frame: u64,
        generation: Generation,
    ) -> Result<Self, PlaybackCommandError> {
        if let Some(current) = current {
            let track = tracks
                .get(current)
                .ok_or(PlaybackCommandError::StartOutOfRange {
                    start: current,
                    tracks: tracks.len(),
                })?;
            if frame > track.total_frames {
                return Err(PlaybackCommandError::FrameOutOfRange {
                    frame,
                    total_frames: track.total_frames,
                });
            }
        } else if frame != 0 {
            return Err(PlaybackCommandError::PositionWithoutTrack { frame });
        }
        Ok(Self {
            tracks,
            current,
            frame,
            generation,
        })
    }

    #[must_use]
    pub fn generation(&self) -> Generation {
        self.generation
    }

    #[must_use]
    pub fn snapshot(&self) -> QueueSnapshot<K> {
        QueueSnapshot {
            tracks: self.tracks.clone(),
            position: self.position(),
        }
    }

    pub fn replace(
        &mut self,
        tracks: Vec<QueueTrack<K>>,
        start: usize,
    ) -> Result<QueueTransition<K>, PlaybackCommandError> {
        if tracks.is_empty() {
            return Err(PlaybackCommandError::EmptyReplacement);
        }
        if start >= tracks.len() {
            return Err(PlaybackCommandError::StartOutOfRange {
                start,
                tracks: tracks.len(),
            });
        }
        self.tracks = tracks;
        self.current = Some(start);
        self.frame = 0;
        self.bump_generation();
        Ok(self.transition())
    }

    pub fn append(&mut self, tracks: Vec<QueueTrack<K>>) -> QueueTransition<K> {
        let prior_length = self.tracks.len();
        let should_start = self.current.is_none() && !tracks.is_empty();
        self.tracks.extend(tracks);
        if should_start {
            self.current = Some(prior_length);
            self.frame = 0;
            self.bump_generation();
        }
        self.transition()
    }

    pub fn truncate(&mut self, length: usize) -> Result<QueueTransition<K>, PlaybackCommandError> {
        if length > self.tracks.len() {
            return Err(PlaybackCommandError::TruncateOutOfRange {
                requested: length,
                tracks: self.tracks.len(),
            });
        }
        self.tracks.truncate(length);
        if self.current.is_some_and(|current| current >= length) {
            self.current = None;
            self.frame = 0;
            self.bump_generation();
        }
        Ok(self.transition())
    }

    pub fn seek(
        &mut self,
        expected_generation: Generation,
        frame: u64,
    ) -> Result<QueueTransition<K>, PlaybackCommandError> {
        if expected_generation != self.generation {
            return Err(PlaybackCommandError::StaleGeneration {
                expected: expected_generation,
                actual: self.generation,
            });
        }
        let current = self.current.ok_or(PlaybackCommandError::NoCurrentTrack)?;
        let total_frames = self.tracks[current].total_frames;
        if frame > total_frames {
            return Err(PlaybackCommandError::FrameOutOfRange {
                frame,
                total_frames,
            });
        }
        self.frame = frame;
        self.bump_generation();
        Ok(self.transition())
    }

    pub fn next_track(&mut self) -> Result<QueueTransition<K>, PlaybackCommandError> {
        let current = self.current.ok_or(PlaybackCommandError::NoCurrentTrack)?;
        self.current = (current + 1 < self.tracks.len()).then_some(current + 1);
        self.frame = 0;
        self.bump_generation();
        Ok(self.transition())
    }

    pub fn previous_track(&mut self) -> Result<QueueTransition<K>, PlaybackCommandError> {
        if self.tracks.is_empty() {
            return Err(PlaybackCommandError::NoCurrentTrack);
        }
        self.current = Some(
            self.current
                .map_or(self.tracks.len() - 1, |current| current.saturating_sub(1)),
        );
        self.frame = 0;
        self.bump_generation();
        Ok(self.transition())
    }

    fn bump_generation(&mut self) {
        self.generation.0 = self.generation.0.wrapping_add(1);
    }

    fn position(&self) -> Option<TrackPosition<K>> {
        let track = self.tracks.get(self.current?)?;
        Some(TrackPosition {
            track_key: track.key.clone(),
            frame: self.frame,
            generation: self.generation,
        })
    }

    fn transition(&self) -> QueueTransition<K> {
        QueueTransition {
            generation: self.generation,
            position: self.position(),
        }
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackCommandError {
    #[error("replacement queue must contain at least one track")]
    EmptyReplacement,
    #[error("start index {start} exceeds queue length {tracks}")]
    StartOutOfRange { start: usize, tracks: usize },
    #[error("truncate length {requested} exceeds queue length {tracks}")]
    TruncateOutOfRange { requested: usize, tracks: usize },
    #[error("queue has no current track")]
    NoCurrentTrack,
    #[error("frame {frame} cannot be restored without a current track")]
    PositionWithoutTrack { frame: u64 },
    #[error("seek generation {expected:?} is stale; current generation is {actual:?}")]
    StaleGeneration {
        expected: Generation,
        actual: Generation,
    },
    #[error("frame {frame} exceeds track length {total_frames}")]
    FrameOutOfRange { frame: u64, total_frames: u64 },
}
