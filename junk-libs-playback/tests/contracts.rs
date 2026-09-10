use junk_libs_playback::{Generation, QueueTrack, TrackPosition};

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConsumerKey(u64);

#[test]
fn consumer_keys_stay_opaque_and_positions_are_track_local_frames() {
    let track = QueueTrack::new(ConsumerKey(42), 1_000);
    let position = TrackPosition::new(&track, 800, Generation(3)).unwrap();
    assert_eq!(position.track_key, ConsumerKey(42));
    assert_eq!(position.frame, 800);
    assert_eq!(position.generation, Generation(3));
    assert!(TrackPosition::new(&track, 1_001, Generation(3)).is_err());
}
