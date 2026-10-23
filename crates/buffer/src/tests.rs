use crate::{BufferId, Cursor, EditTransaction, SearchEngine, TextBuffer, TextEdit};
use nozhik_event_bus::{BufferEvent, Event, EventBus};
use std::{error::Error, time::Instant};

type TestResult = std::result::Result<(), Box<dyn Error>>;

fn buffer(text: &str) -> TextBuffer {
    TextBuffer::load_from_str(BufferId(1), text)
}

fn edit(start: usize, end: usize, text: &str) -> TextEdit {
    TextEdit {
        range: start..end,
        text: text.into(),
    }
}

#[test]
fn unicode_navigation_and_edits() -> TestResult {
    let mut buffer = buffer("Я😀e\u{301}\r\nстрока\n");
    for index in 0..=buffer.rope().len_chars() {
        let position = buffer.char_to_position(index)?;
        assert_eq!(
            buffer.line_col_to_char(position.line as usize, position.column as usize)?,
            index
        );
        assert_eq!(buffer.position_at(position.byte_offset)?, position);
    }
    assert_eq!(buffer.line(0)?.to_string(), "Я😀e\u{301}\r\n");
    assert_eq!(buffer.rope().len_lines(), 3);
    assert!(buffer.insert(1, "x").is_err());
    assert!(buffer.delete(2..3).is_err());
    assert!(buffer.position_at(usize::MAX).is_err());
    assert!(buffer.char_to_position(usize::MAX).is_err());
    assert!(buffer.line_col_to_char(0, 11).is_err());
    assert!(buffer.line_col_to_char(3, 0).is_err());
    assert!(buffer.line_col_to_char(0, usize::MAX).is_err());
    buffer.delete(2..6)?;
    buffer.insert(2, "🦀")?;
    assert_eq!(buffer.line(0)?.to_string(), "Я🦀e\u{301}\r\n");
    Ok(())
}

#[test]
fn insert_undo_redo_and_save_branch() -> TestResult {
    let mut buffer = buffer("abc");
    assert!(!buffer.is_dirty());
    buffer.insert(1, "😀")?;
    assert!(buffer.is_dirty());
    buffer.undo()?;
    assert_eq!(buffer.rope().to_string(), "abc");
    assert!(!buffer.is_dirty());
    buffer.redo()?;
    assert_eq!(buffer.rope().to_string(), "a😀bc");
    buffer.mark_saved();
    buffer.undo()?;
    assert!(buffer.is_dirty());
    buffer.redo()?;
    assert!(!buffer.is_dirty());
    buffer.undo()?;
    buffer.insert(0, "z")?;
    assert!(buffer.redo().is_err());
    assert!(buffer.is_dirty());
    Ok(())
}

#[test]
fn transactions_are_atomic_and_inverse_edits_restore_text() -> TestResult {
    let mut buffer = buffer("0123456789");
    for edits in [
        vec![edit(0, 1, "x"), edit(99, 99, "y")],
        vec![edit(1, 5, "a"), edit(4, 6, "b")],
        vec![edit(2, 2, "a"), edit(2, 2, "b")],
        vec![edit(4, 2, "x")],
    ] {
        assert!(
            buffer
                .apply_transaction(EditTransaction { edits, group: None })
                .is_err()
        );
        assert_eq!(buffer.rope().to_string(), "0123456789");
        assert_eq!(buffer.snapshot().revision, 0);
        assert!(!buffer.is_dirty());
    }
    buffer.apply_transaction(EditTransaction {
        edits: vec![edit(1, 3, "Я"), edit(5, 8, "😀"), edit(8, 8, "!")],
        group: None,
    })?;
    assert_eq!(buffer.rope().to_string(), "0Я34😀!89");
    buffer.undo()?;
    assert_eq!(buffer.rope().to_string(), "0123456789");
    buffer.redo()?;
    assert_eq!(buffer.rope().to_string(), "0Я34😀!89");
    Ok(())
}

#[test]
fn explicit_groups_and_save_boundaries() -> TestResult {
    let mut buffer = buffer("");
    for (offset, text) in [(0, "a"), (1, "b")] {
        buffer.apply_transaction(EditTransaction {
            edits: vec![edit(offset, offset, text)],
            group: Some(7),
        })?;
    }
    buffer.undo()?;
    assert_eq!(buffer.rope().len_bytes(), 0);
    buffer.redo()?;
    assert_eq!(buffer.rope().to_string(), "ab");
    buffer.mark_saved();
    buffer.apply_transaction(EditTransaction {
        edits: vec![edit(2, 2, "c")],
        group: Some(7),
    })?;
    buffer.undo()?;
    assert!(!buffer.is_dirty());
    assert_eq!(buffer.rope().to_string(), "ab");
    Ok(())
}

#[test]
fn cursors_merge_preserve_direction_and_restore() -> TestResult {
    let mut buffer = buffer("abcdefghij");
    let cursor = |anchor, head| -> crate::Result<Cursor> {
        Ok(Cursor {
            anchor: buffer.position_at(anchor)?,
            head: buffer.position_at(head)?,
        })
    };
    let selections = vec![cursor(5, 1)?, cursor(4, 8)?, cursor(8, 8)?];
    buffer.set_cursors(selections)?;
    let merged = buffer.cursors().as_slice().first().ok_or("no cursor")?;
    assert_eq!((merged.anchor.byte_offset, merged.head.byte_offset), (8, 1));
    assert_eq!(buffer.cursors().as_slice().len(), 1);
    let selections = vec![
        Cursor {
            anchor: buffer.position_at(2)?,
            head: buffer.position_at(2)?,
        },
        Cursor {
            anchor: buffer.position_at(6)?,
            head: buffer.position_at(6)?,
        },
    ];
    buffer.set_cursors(selections)?;
    buffer.delete(1..7)?;
    assert_eq!(buffer.cursors().as_slice().len(), 1);
    assert_eq!(
        buffer
            .cursors()
            .as_slice()
            .first()
            .ok_or("no cursor")?
            .head
            .byte_offset,
        1
    );
    buffer.undo()?;
    assert_eq!(buffer.cursors().as_slice().len(), 2);
    assert!(buffer.set_cursors(vec![]).is_err());
    let mut stale = buffer.position_at(0)?;
    stale.column = 1;
    assert!(
        buffer
            .set_cursors(vec![Cursor {
                anchor: stale,
                head: stale
            }])
            .is_err()
    );
    Ok(())
}

#[test]
fn search_unicode_multiline_empty_and_chunk_boundaries() -> TestResult {
    let text = format!("{}Я😀\nконец\r\n{}", "a".repeat(4095), "z".repeat(4096));
    let buffer = buffer(&text);
    let pattern = "aЯ😀\nконец";
    let expected = 4094..4094 + pattern.len();
    assert_eq!(
        SearchEngine::literal(pattern)?.find_all(&buffer),
        vec![expected.clone()]
    );
    assert_eq!(
        SearchEngine::regex("aЯ.\\nконец")?.find_all(&buffer),
        vec![expected]
    );
    assert_eq!(
        SearchEngine::regex("(?m)^конец\\r?$")?
            .find_all(&buffer)
            .len(),
        1
    );
    assert_eq!(SearchEngine::literal(".")?.find_all(&buffer).len(), 0);
    assert!(SearchEngine::regex("[").is_err());
    let tiny = self::buffer("Я😀");
    assert_eq!(
        SearchEngine::regex("")?.find_all(&tiny),
        vec![0..0, 2..2, 6..6]
    );
    assert_eq!(
        SearchEngine::regex("\\A\\z")?.find_all(&self::buffer("")),
        vec![0..0]
    );
    assert_eq!(
        SearchEngine::literal(&text)?.find_all(&buffer),
        vec![0..text.len()]
    );
    Ok(())
}

#[test]
fn events_commit_order_replay_and_noops() -> TestResult {
    let bus = EventBus::new(32)?;
    let mut receiver = bus.subscribe();
    let mut buffer = buffer("abcdef");
    buffer.set_event_bus(bus);
    buffer.insert(0, "")?;
    assert!(buffer.insert(99, "x").is_err());
    assert!(receiver.try_recv().is_err());
    let mut replay = String::from("abcdef");
    buffer.apply_transaction(EditTransaction {
        edits: vec![edit(0, 1, "Я"), edit(4, 6, "😀")],
        group: Some(1),
    })?;
    buffer.apply_transaction(EditTransaction {
        edits: vec![edit(2, 2, "!")],
        group: Some(1),
    })?;
    buffer.undo()?;
    buffer.redo()?;
    for revision in 1..=4 {
        let event = receiver.try_recv()?;
        if let Event::Buffer(BufferEvent::Modified {
            buffer_id,
            revision: actual,
            edits,
        }) = &*event
        {
            assert_eq!(*buffer_id, BufferId(1));
            assert_eq!(*actual, revision);
            for edit in edits {
                replay.replace_range(edit.range.clone(), &edit.text);
            }
        } else {
            return Err("expected text event".into());
        }
        assert!(matches!(
            &*receiver.try_recv()?,
            Event::Buffer(BufferEvent::CursorChanged { .. })
        ));
    }
    assert_eq!(replay, buffer.rope().to_string());
    let caret = buffer.position_at(0)?;
    buffer.set_cursors(vec![Cursor {
        anchor: caret,
        head: caret,
    }])?;
    assert!(matches!(
        &*receiver.try_recv()?,
        Event::Buffer(BufferEvent::CursorChanged { .. })
    ));
    Ok(())
}

#[test]
fn detached_history_rejects_foreign_and_stale_buffers() -> TestResult {
    let mut first = buffer("a");
    first.insert(1, "b")?;
    let mut history = first.take_history();
    let mut other = buffer("a");
    other.insert(1, "b")?;
    assert!(history.undo(&mut other).is_err());
    history.undo(&mut first)?;
    history.redo(&mut first)?;
    first.insert(2, "c")?;
    assert!(history.undo(&mut first).is_err());
    Ok(())
}

#[test]
fn empty_buffer_viewports_and_noops() -> TestResult {
    let mut buffer = buffer("");
    assert_eq!(buffer.line(0)?.len_bytes(), 0);
    assert_eq!(buffer.lines(0..1)?.len_bytes(), 0);
    assert_eq!(buffer.lines(1..1)?.len_bytes(), 0);
    assert!(buffer.lines(std::ops::Range { start: 1, end: 0 }).is_err());
    assert!(buffer.lines(0..2).is_err());
    buffer.delete(0..0)?;
    assert!(buffer.undo().is_err());
    buffer.insert(0, "a\rb\r\nc\n")?;
    assert_eq!(buffer.rope().len_lines(), 3);
    Ok(())
}

#[test]
fn fifty_thousand_lines_share_snapshots_and_keep_edits_local() -> TestResult {
    let text = "строка 😀 0123456789\n".repeat(50_000);
    let start = Instant::now();
    let mut buffer = buffer(&text);
    assert_eq!(buffer.rope().len_lines(), 50_001);
    let snapshot = buffer.snapshot();
    // Pointer equality proves that taking a snapshot did not copy rope chunks.
    assert_eq!(
        snapshot.text.chunks().next().map(str::as_ptr),
        buffer.rope().chunks().next().map(str::as_ptr)
    );
    let offset = buffer.rope().line_to_byte(25_000);
    buffer.insert(offset, "new ")?;
    assert_eq!(
        buffer.lines(25_000..25_001)?.to_string(),
        "new строка 😀 0123456789\n"
    );
    assert_eq!(snapshot.text.len_bytes(), text.len());
    assert_eq!(
        snapshot.text.chunks().next().map(str::as_ptr),
        buffer.rope().chunks().next().map(str::as_ptr)
    );
    assert_eq!(
        SearchEngine::literal("строка")?.find_all(&buffer).len(),
        50_000
    );
    buffer.undo()?;
    assert_eq!(buffer.rope(), &snapshot.text);
    // Generous regression guard, not a hardware-independent latency guarantee.
    assert!(start.elapsed().as_secs() < 30);
    eprintln!(
        "50,000 lines, {} bytes, load/snapshot/edit/search/undo: {:?}",
        text.len(),
        start.elapsed()
    );
    Ok(())
}

#[test]
fn save_and_cursor_movement_close_open_groups() -> TestResult {
    let mut buffer = buffer("");
    let grouped = |offset, text| EditTransaction {
        edits: vec![edit(offset, offset, text)],
        group: Some(42),
    };
    buffer.apply_transaction(grouped(0, "a"))?;
    buffer.mark_saved();
    buffer.apply_transaction(grouped(1, "b"))?;
    buffer.undo()?;
    assert_eq!(buffer.rope().to_string(), "a");
    assert!(!buffer.is_dirty());
    buffer.redo()?;
    buffer.apply_transaction(grouped(2, "c"))?;
    let position = buffer.position_at(0)?;
    buffer.set_cursors(vec![Cursor {
        anchor: position,
        head: position,
    }])?;
    buffer.apply_transaction(grouped(3, "d"))?;
    buffer.undo()?;
    assert_eq!(buffer.rope().to_string(), "abc");
    buffer.undo()?;
    assert_eq!(buffer.rope().to_string(), "ab");
    Ok(())
}

#[test]
fn exhausted_revision_is_atomic_in_both_edit_and_history() -> TestResult {
    let mut buffer = buffer("a");
    buffer.insert(1, "b")?;
    buffer.revision = u64::MAX;
    let before = buffer.snapshot();
    assert!(buffer.insert(0, "x").is_err());
    assert!(buffer.undo().is_err());
    assert_eq!(buffer.rope(), &before.text);
    assert_eq!(buffer.cursors(), &before.cursors);
    buffer.revision = 1;
    buffer.undo()?; // failure did not remove the history entry
    assert_eq!(buffer.rope().to_string(), "a");
    Ok(())
}

#[test]
fn unicode_transaction_sequence_matches_string_model_and_event_replay() -> TestResult {
    let mut buffer = buffer("Я😀e\u{301}\r\nначало\n");
    let mut model = buffer.rope().to_string();
    let bus = EventBus::new(8)?;
    let mut receiver = bus.subscribe();
    buffer.set_event_bus(bus);
    let mut states = vec![model.clone()];
    for step in 0..128 {
        let boundaries: Vec<_> = model
            .char_indices()
            .map(|(offset, _)| offset)
            .chain(std::iter::once(model.len()))
            .collect();
        let middle = boundaries.len() / 2;
        let left = *boundaries
            .get(step % middle)
            .ok_or("missing left boundary")?;
        let right = *boundaries
            .get(middle + step % (boundaries.len() - middle))
            .ok_or("missing right boundary")?;
        let replacement = if step % 2 == 0 { "🦀\n" } else { "й\u{301}" };
        let edits = vec![edit(left, left, replacement), edit(right, model.len(), "Z")];
        let mut replay = model.clone();
        model.replace_range(right.., "Z");
        model.insert_str(left, replacement);
        buffer.apply_transaction(EditTransaction { edits, group: None })?;
        let event = receiver.try_recv()?;
        if let Event::Buffer(BufferEvent::Modified { edits, .. }) = &*event {
            for edit in edits {
                replay.replace_range(edit.range.clone(), &edit.text);
            }
        } else {
            return Err("expected modified event".into());
        }
        assert!(matches!(
            &*receiver.try_recv()?,
            Event::Buffer(BufferEvent::CursorChanged { .. })
        ));
        assert_eq!(buffer.rope().to_string(), model);
        assert_eq!(replay, model);
        states.push(model.clone());
    }
    for expected in states.iter().rev().skip(1) {
        buffer.undo()?;
        let event = receiver.try_recv()?;
        if let Event::Buffer(BufferEvent::Modified { edits, .. }) = &*event {
            for edit in edits {
                model.replace_range(edit.range.clone(), &edit.text);
            }
        } else {
            return Err("expected inverse event".into());
        }
        assert!(matches!(
            &*receiver.try_recv()?,
            Event::Buffer(BufferEvent::CursorChanged { .. })
        ));
        assert_eq!(buffer.rope().to_string(), *expected);
        assert_eq!(model, *expected);
    }
    assert!(!buffer.is_dirty());
    assert!(buffer.undo().is_err());
    Ok(())
}
