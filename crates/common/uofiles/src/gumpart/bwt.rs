//! The second compression pass gump art layers on top of zlib.
//!
//! Ported from ClassicUO's `BwtDecompress.Decompress` and
//! `BwtDecompress.InternalDecompress`
//! (`src/ClassicUO.Utility/BwtDecompress.cs`). The class name says
//! Burrows-Wheeler; the algorithm is not a BWT inverse. Stage one
//! ([`move_to_front_decode`]) is a plain move-to-front decode. Stage two
//! ([`expand_runs`]) reads a per-symbol frequency table and a stream of
//! "where each symbol's occurrences are, in the order they appear" and
//! expands that back into a flat byte stream. Whatever the scheme is actually
//! called, this is what the retail client runs on every gump before there is
//! a width, a height, or a pixel anywhere in the data — checked against a
//! shipped `gumpartLegacyMUL.uop` in `super::tests`.
//!
//! Every array size and offset below (256, 1024, `+ 256`, `+ 512`) is the
//! reference's own arithmetic, kept rather than re-derived: a value this
//! specific getting re-justified from first principles is a value someone
//! will "simplify" back to the wrong number later.

use openshard_protocol::wire::Graphic;

use super::GumpError;

/// Undo both stages: move-to-front, then the run expansion underneath it.
pub(super) fn decode(graphic: Graphic, data: &[u8]) -> Result<Vec<u8>, GumpError> {
    let stage1 = move_to_front_decode(graphic, data)?;
    expand_runs(graphic, &stage1)
}

/// Stage one: `BwtDecompress.Decompress`.
///
/// The input opens with a 4-byte value this reader has no use for (the
/// reference reads it into a local and never touches it again — `Decompress`
/// takes zlib's output directly, and zlib's own length already told the
/// caller how much of it there is) and then one byte that seeds the decode.
///
/// # The 65536-entry table is the identity permutation, always
///
/// The reference builds its move-to-front table by writing `(firstByte,
/// secondByte)` pairs — every value `0..=65535` exactly once, just starting
/// the cycle at the seed byte instead of at zero — and then sorting the whole
/// table. Sorting a set that already contains every value in a range restores
/// the same order no matter where the cycle started, so the seed has no
/// effect on anything downstream. Only indices `0..256` are ever read again
/// after that, and sorted ascending they are `0, 1, 2, …, 255`: the identity
/// byte order. So this reader skips building and sorting 65536 entries and
/// starts directly at the table that sort would have produced.
fn move_to_front_decode(graphic: Graphic, data: &[u8]) -> Result<Vec<u8>, GumpError> {
    let malformed = |detail: &str| {
        GumpError::Malformed {
            graphic,
            detail: detail.to_owned(),
        }
    };

    // 4 bytes discarded, 1 byte read as the seed for the loop below.
    if data.len() < 5 {
        return Err(malformed("shorter than its own 5-byte prologue"));
    }
    let mut pos = 4usize;
    let mut current_byte = data[pos];
    pos += 1;

    let mut table = [0u8; 256];
    for (index, slot) in table.iter_mut().enumerate() {
        *slot = index as u8;
    }

    // The reference allocates `data.len() - 4` slots, though the loop below —
    // reading one further byte per iteration as the *next* iteration's input
    // — only ever fills `data.len() - 5` of them. The last slot is read by
    // stage two as ordinary data, not specially skipped, so it is kept at its
    // Rust-default zero to match the reference's zero-initialised array
    // rather than trimmed away.
    let mut out = vec![0u8; data.len() - 4];
    let mut written = 0usize;
    while pos < data.len() {
        // `current_byte` doubles as the move-to-front index to read *and* the
        // byte value that lands at the front of the table afterwards —
        // that's what makes this move-to-front and not a plain substitution:
        // the table's arrangement itself is the running state.
        let mut index = current_byte as usize;
        let value = table[index];
        while index > 0 {
            table[index] = table[index - 1];
            index -= 1;
        }
        table[0] = value;

        out[written] = value;
        written += 1;
        current_byte = data[pos];
        pos += 1;
    }
    Ok(out)
}

/// Stage two: `BwtDecompress.InternalDecompress`.
///
/// The move-to-front output opens with its own header: 256 little-endian
/// `i32` counts — how many times each byte value `0..=255` appears in the
/// final output — followed at byte 1024 by a stream that says, for each
/// symbol in descending order of count, which *other* symbol follows each of
/// its occurrences (or `0` for "the same symbol continues"). Expanding that
/// back into a flat byte stream is what this function does.
fn expand_runs(graphic: Graphic, input: &[u8]) -> Result<Vec<u8>, GumpError> {
    let malformed = |detail: String| GumpError::Malformed { graphic, detail };

    let header = input
        .get(..1024)
        .ok_or_else(|| malformed("shorter than its own 1024-byte count table".to_owned()))?;
    let mut counts = [0i32; 256];
    for (index, word) in header.as_chunks::<4>().0.iter().enumerate() {
        counts[index] = i32::from_le_bytes(*word);
    }

    let total: i64 = counts.iter().map(|&count| i64::from(count)).sum();
    let length = usize::try_from(total)
        .map_err(|_| malformed(format!("the count table sums to {total}, not a byte count")))?;

    // `Frequency` in the reference: repeatedly takes the largest remaining
    // count, in place, until every nonzero one has been taken. This is *not*
    // the same as `counts` sorted descending by value — ties break by
    // whichever index the scan reaches first, and that break has to match
    // what encoded the file or the symbol order that follows comes out
    // scrambled. A `sort_by_key` here would tie-break by a different rule
    // (stability, or reverse index order) and silently decode a plausible,
    // wrong picture.
    let mut remaining = counts;
    let mut order = Vec::new();
    for _ in 0..256 {
        let mut best_index = 0usize;
        let mut best_count = 0i32;
        for (index, &count) in remaining.iter().enumerate() {
            if count > best_count {
                best_index = index;
                best_count = count;
            }
        }
        if best_count == 0 {
            break;
        }
        order.push(best_index as u8);
        remaining[best_index] = 0;
    }

    // For each symbol: `cursor` is the next unread position in the
    // `input[1024..]` "what follows" stream for that symbol's occurrences,
    // and `cursor_end` is one past its last. The reference reuses one
    // 768-entry array (`partialInput`) for the counts and both of these,
    // at a fixed `+256`/`+512` offset; kept as three named arrays here
    // instead, since nothing downstream needs them contiguous.
    let mut cursor = [0usize; 256];
    let mut cursor_end = [0usize; 256];
    let mut symbol_table = [0u8; 256];
    for (index, slot) in symbol_table.iter_mut().enumerate() {
        *slot = index as u8;
    }

    let non_zero_count = order.len();
    let mut position = 0usize;
    for &symbol in &order {
        let next_symbol = *input
            .get(1024 + position)
            .ok_or_else(|| malformed(format!("the position stream runs out at symbol {symbol}")))?;
        symbol_table[next_symbol as usize] = symbol;
        cursor[symbol as usize] = position + 1;
        let count = usize::try_from(counts[symbol as usize])
            .map_err(|_| malformed(format!("symbol {symbol} has a negative count")))?;
        position += count;
        cursor_end[symbol as usize] = position;
    }

    let mut output = vec![0u8; length];
    if length == 0 {
        return Ok(output);
    }

    let mut active = non_zero_count;
    let mut current = symbol_table[0];
    let mut written = 0usize;
    loop {
        output[written] = current;

        if cursor[current as usize] >= cursor_end[current as usize] {
            // `current`'s run is exhausted: drop it from the front of the
            // active list and move on to whatever is now at the front.
            // `active` reaching 0 here means the data lied about its own
            // length — the reference keeps decrementing and reading `val`
            // unmodified in that case, which this mirrors by simply not
            // touching `current` again; the length check below is what
            // actually stops the loop on such input rather than looping past
            // the output buffer.
            if active > 0 {
                active -= 1;
                shift_left(&mut symbol_table, active);
                current = symbol_table[0];
            }
        } else {
            let at = cursor[current as usize];
            let next_symbol = *input
                .get(1024 + at)
                .ok_or_else(|| malformed("the position stream runs out mid-symbol".to_owned()))?;
            cursor[current as usize] = at + 1;
            if next_symbol != 0 {
                // Promote `next_symbol`'s slot to the front, the same
                // move-to-front step stage one uses, and hand `current`
                // the slot it vacates.
                shift_left(&mut symbol_table, next_symbol as usize);
                symbol_table[next_symbol as usize] = current;
                current = symbol_table[0];
            }
        }

        written += 1;
        if written >= length {
            break;
        }
    }

    Ok(output)
}

/// Shift `table[0..max]` left by one, discarding `table[0]` and leaving
/// `table[max]` unchanged — the reference's `ShiftLeft`. `max` is always a
/// byte value (`0..=255`) or a count already bounded by `256`, so `max + 1`
/// staying within the table's 256 slots is an invariant of every call site
/// rather than something this function re-checks.
fn shift_left(table: &mut [u8; 256], max: usize) {
    for i in 0..max {
        table[i] = table[i + 1];
    }
}
