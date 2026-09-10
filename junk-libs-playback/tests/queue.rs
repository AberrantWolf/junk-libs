use junk_libs_playback::{Generation, PlaybackCommandError, QueueState, QueueTrack};

#[test]
fn queue_transitions_and_track_boundaries_are_deterministic() {
    let mut queue = QueueState::new();
    let transition = queue
        .replace(vec![QueueTrack::new("a", 10), QueueTrack::new("b", 20)], 0)
        .unwrap();
    assert_eq!(transition.generation, Generation(1));
    assert_eq!(transition.position.unwrap().track_key, "a");

    queue.seek(Generation(1), 9).unwrap();
    assert!(matches!(
        queue.seek(Generation(1), 4),
        Err(PlaybackCommandError::StaleGeneration { .. })
    ));
    let next = queue.next_track().unwrap();
    assert_eq!(next.position.unwrap().track_key, "b");
    assert_eq!(
        queue.previous_track().unwrap().position.unwrap().track_key,
        "a"
    );

    let generation = queue.generation();
    queue.append(vec![QueueTrack::new("c", 30)]);
    assert_eq!(queue.generation(), generation);
    queue.truncate(1).unwrap();
    assert_eq!(queue.snapshot().tracks.len(), 1);
}

#[test]
fn append_after_exhaustion_resumes_at_the_new_track() {
    let mut queue = QueueState::new();
    queue.replace(vec![QueueTrack::new("done", 1)], 0).unwrap();
    assert!(queue.next_track().unwrap().position.is_none());

    let resumed = queue.append(vec![QueueTrack::new("new", 2)]);
    assert_eq!(resumed.position.unwrap().track_key, "new");
    assert_eq!(
        queue.previous_track().unwrap().position.unwrap().track_key,
        "done"
    );
}

#[test]
fn adapter_state_restores_without_changing_generation() {
    let queue = QueueState::restore(
        vec![QueueTrack::new("audible", 100)],
        Some(0),
        40,
        Generation(9),
    )
    .unwrap();
    assert_eq!(queue.generation(), Generation(9));
    assert_eq!(queue.snapshot().position.unwrap().frame, 40);
    assert!(QueueState::restore(vec![QueueTrack::new("x", 1)], None, 1, Generation(0)).is_err());
}

#[test]
fn replace_and_bounds_fail_without_partial_mutation() {
    let mut queue = QueueState::new();
    assert!(queue.replace(vec![], 0).is_err());
    queue.replace(vec![QueueTrack::new(7_u64, 8)], 0).unwrap();
    let before = queue.snapshot();
    assert!(queue.seek(queue.generation(), 9).is_err());
    assert_eq!(queue.snapshot(), before);
    assert!(queue.replace(vec![QueueTrack::new(8, 1)], 1).is_err());
    assert_eq!(queue.snapshot(), before);
}
