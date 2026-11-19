//! The text of a text field, split up the way AccessKit wants it.
//!
//! Flutter describes a text field as one string and a selection, in UTF-16
//! code units from its start. AccessKit wants the text as a list of runs,
//! each a child node of the field, and a selection as a run and the index of
//! a character in it.

use accesskit::NodeId;

/// AccessKit counts the characters and the word starts of a run in a byte.
const MAX_RUN_LENGTH: usize = u8::MAX as usize;

/// The id of a run of a text field.
///
/// Flutter's ids are 32 bit. The upper half of the ids of the runs says
/// which run of the field it is, starting above the half that
/// [`WINDOW_NODE_ID`](crate::tree::WINDOW_NODE_ID) is in.
pub(crate) fn text_run_id(field: i32, run: usize) -> NodeId {
    NodeId(((run as u64 + 2) << 32) | field as u32 as u64)
}

/// The text field a run belongs to, and which of its runs it is.
pub(crate) fn text_run_of(id: NodeId) -> Option<(i32, usize)> {
    let run = (id.0 >> 32).checked_sub(2)?;
    let field = i32::try_from(id.0 & 0xffff_ffff).ok()?;
    Some((field, run as usize))
}

/// A stretch of text that is on one line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct TextRun {
    /// With the line break at its end, if the line ends with one.
    pub text: String,
    /// The length of each character in UTF-8 code units.
    pub character_lengths: Vec<u8>,
    /// The length of each character in UTF-16 code units.
    utf16_lengths: Vec<u8>,
    /// The indices of the characters that start a word.
    pub word_starts: Vec<u8>,
    /// Whether this run goes on where the one before it stopped, on the
    /// same line.
    pub continues_line: bool,
}

impl TextRun {
    fn utf16_length(&self) -> usize {
        self.utf16_lengths
            .iter()
            .map(|length| *length as usize)
            .sum()
    }

    fn ends_line(&self) -> bool {
        self.text.ends_with('\n')
    }
}

/// The text of a text field as runs.
///
/// There is a run for every line, and more than one for a line that is too
/// long for one. Where the lines are wrapped on screen is not known: a line
/// here ends where the text has a line break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextLayout {
    /// At least one, so that there is a place for the caret in a field
    /// without text.
    pub runs: Vec<TextRun>,
}

impl TextLayout {
    pub fn new(text: &str) -> Self {
        let mut runs: Vec<TextRun> = Vec::new();
        let mut run = TextRun::default();
        let mut after_whitespace = false;

        let mut chars = text.chars().peekable();
        while let Some(char) = chars.next() {
            // A CRLF is one character: there is no place for a caret in
            // the middle of it.
            let is_crlf = char == '\r' && chars.peek() == Some(&'\n');
            if is_crlf {
                chars.next();
            }
            let is_line_break = char == '\n' || is_crlf;

            if run.character_lengths.len() == MAX_RUN_LENGTH {
                runs.push(std::mem::take(&mut run));
                run.continues_line = true;
            }

            let index = run.character_lengths.len();
            let is_whitespace = char.is_whitespace();
            // The first character of a line starts a word without being
            // listed. Whitespace belongs to the word before it.
            let is_line_start = index == 0 && !run.continues_line;
            if !is_line_start && !is_whitespace && after_whitespace {
                run.word_starts.push(index as u8);
            }
            after_whitespace = is_whitespace;

            if is_crlf {
                run.text.push_str("\r\n");
                run.character_lengths.push(2);
                run.utf16_lengths.push(2);
            } else {
                run.text.push(char);
                run.character_lengths.push(char.len_utf8() as u8);
                run.utf16_lengths.push(char.len_utf16() as u8);
            }

            if is_line_break {
                runs.push(std::mem::take(&mut run));
                after_whitespace = false;
            }
        }

        // The last line, which is empty if the text ends with a line break
        // or if there is no text at all: the caret can be there.
        let is_last_line_open = runs.last().is_none_or(|last| last.ends_line());
        if !run.character_lengths.is_empty() || is_last_line_open {
            runs.push(run);
        }

        Self { runs }
    }

    /// The run and the character in it that are at an offset in UTF-16 code
    /// units from the start of the text.
    ///
    /// An offset past the end is the end. One in the middle of a character
    /// is the start of that character.
    pub fn position(&self, utf16_offset: usize) -> (usize, usize) {
        let mut remaining = utf16_offset;

        for (index, run) in self.runs.iter().enumerate() {
            let length = run.utf16_length();
            let is_last = index + 1 == self.runs.len();
            // The end of a run is the start of the next one.
            if remaining < length || is_last {
                let mut character = 0;
                for character_length in &run.utf16_lengths {
                    if remaining < *character_length as usize {
                        break;
                    }
                    remaining -= *character_length as usize;
                    character += 1;
                }
                return (index, character);
            }
            remaining -= length;
        }

        unreachable!("a text layout has at least one run")
    }

    /// The reverse of [`position`](Self::position).
    pub fn utf16_offset(&self, run: usize, character: usize) -> Option<usize> {
        let target = self.runs.get(run)?;
        if character > target.utf16_lengths.len() {
            return None;
        }

        let before: usize = self.runs[..run].iter().map(TextRun::utf16_length).sum();
        let within: usize = target.utf16_lengths[..character]
            .iter()
            .map(|length| *length as usize)
            .sum();
        Some(before + within)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(layout: &TextLayout) -> Vec<&str> {
        layout.runs.iter().map(|run| run.text.as_str()).collect()
    }

    #[test]
    fn run_ids() {
        assert_eq!(text_run_of(text_run_id(7, 0)), Some((7, 0)));
        assert_eq!(
            text_run_of(text_run_id(i32::MAX, 300)),
            Some((i32::MAX, 300))
        );
        assert_ne!(text_run_id(0, 0), crate::tree::WINDOW_NODE_ID);
        assert_eq!(text_run_of(crate::tree::WINDOW_NODE_ID), None);
        assert_eq!(text_run_of(NodeId(7)), None);
    }

    #[test]
    fn no_text_is_one_empty_run() {
        let layout = TextLayout::new("");

        assert_eq!(texts(&layout), [""]);
        assert!(layout.runs[0].character_lengths.is_empty());
        assert_eq!(layout.position(0), (0, 0));
        assert_eq!(layout.position(5), (0, 0));
        assert_eq!(layout.utf16_offset(0, 0), Some(0));
    }

    #[test]
    fn one_line() {
        let layout = TextLayout::new("héllo wörld  again");

        assert_eq!(texts(&layout), ["héllo wörld  again"]);
        let run = &layout.runs[0];
        assert_eq!(run.character_lengths.len(), 18);
        assert_eq!(run.character_lengths[..3], [1, 2, 1]);
        assert_eq!(
            run.character_lengths
                .iter()
                .map(|l| *l as usize)
                .sum::<usize>(),
            run.text.len()
        );
        assert_eq!(run.word_starts, [6, 13]);
        assert!(!run.continues_line);
    }

    #[test]
    fn a_line_break_ends_a_run() {
        let layout = TextLayout::new("one\ntwo three\n");

        assert_eq!(texts(&layout), ["one\n", "two three\n", ""]);
        assert_eq!(layout.runs[0].character_lengths, [1, 1, 1, 1]);
        assert!(layout.runs[0].word_starts.is_empty());
        assert_eq!(layout.runs[1].word_starts, [4]);
        assert!(layout.runs.iter().all(|run| !run.continues_line));
    }

    #[test]
    fn a_crlf_is_one_character() {
        let layout = TextLayout::new("a\r\nb");

        assert_eq!(texts(&layout), ["a\r\n", "b"]);
        assert_eq!(layout.runs[0].character_lengths, [1, 2]);
        assert_eq!(layout.position(1), (0, 1));
        // Between the two halves of the line break: at its start.
        assert_eq!(layout.position(2), (0, 1));
        assert_eq!(layout.position(3), (1, 0));
        assert_eq!(layout.utf16_offset(1, 0), Some(3));
    }

    #[test]
    fn positions_count_utf16_code_units() {
        // The emoji is two UTF-16 code units and four UTF-8 ones.
        let layout = TextLayout::new("a😀b\ncd");

        assert_eq!(layout.runs[0].character_lengths, [1, 4, 1, 1]);
        assert_eq!(layout.position(0), (0, 0));
        assert_eq!(layout.position(1), (0, 1));
        // In the middle of the emoji.
        assert_eq!(layout.position(2), (0, 1));
        assert_eq!(layout.position(3), (0, 2));
        // Before the line break, and after it.
        assert_eq!(layout.position(4), (0, 3));
        assert_eq!(layout.position(5), (1, 0));
        assert_eq!(layout.position(7), (1, 2));
        assert_eq!(layout.position(99), (1, 2));

        assert_eq!(layout.utf16_offset(0, 2), Some(3));
        assert_eq!(layout.utf16_offset(1, 2), Some(7));
        assert_eq!(layout.utf16_offset(1, 3), None);
        assert_eq!(layout.utf16_offset(2, 0), None);
    }

    #[test]
    fn the_caret_after_a_last_line_break_is_on_the_empty_last_line() {
        let layout = TextLayout::new("ab\n");

        assert_eq!(layout.position(2), (0, 2));
        assert_eq!(layout.position(3), (1, 0));
        assert_eq!(layout.utf16_offset(1, 0), Some(3));
    }

    #[test]
    fn a_long_line_is_more_than_one_run() {
        let text = "word ".repeat(120);
        let layout = TextLayout::new(&text);

        assert_eq!(
            layout
                .runs
                .iter()
                .map(|run| run.character_lengths.len())
                .collect::<Vec<_>>(),
            [255, 255, 90]
        );
        assert_eq!(texts(&layout).concat(), text);
        assert!(!layout.runs[0].continues_line);
        assert!(layout.runs[1].continues_line);
        assert!(layout.runs[2].continues_line);
        // The first run ends with "word " and the second starts with a
        // word. The first word of a line is not listed, that of a run that
        // continues one is.
        assert_eq!(layout.runs[0].word_starts[0], 5);
        assert_eq!(layout.runs[1].word_starts[0], 0);

        assert_eq!(layout.position(254), (0, 254));
        assert_eq!(layout.position(255), (1, 0));
        assert_eq!(layout.position(600), (2, 90));
        assert_eq!(layout.utf16_offset(1, 0), Some(255));
        assert_eq!(layout.utf16_offset(2, 90), Some(600));
    }

    #[test]
    fn a_run_that_is_full_is_the_last_one() {
        let layout = TextLayout::new(&"a".repeat(255));

        assert_eq!(layout.runs.len(), 1);
        assert_eq!(layout.position(255), (0, 255));
    }

    #[test]
    fn a_word_that_goes_on_in_the_next_run_does_not_start_there() {
        let layout = TextLayout::new(&"a".repeat(300));

        assert_eq!(layout.runs.len(), 2);
        assert!(layout.runs[1].word_starts.is_empty());
    }
}
