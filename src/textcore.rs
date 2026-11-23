//! Text capture without the operating system: what a capture can end as, the rules WinTextCapture
//! had for selections, image sizes, line breaks, previews and clipboard retries, and the character
//! error rate its measurements use. Everything here runs on Linux too.

use unicode_segmentation::UnicodeSegmentation;

/// How one text capture ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextOutcome {
    /// Esc, right click, or a selection too small to read. Shows nothing.
    Cancelled,
    /// On the clipboard and read back the same.
    Copied(String),
    /// The reader found nothing; the clipboard was not touched.
    NoText,
    /// Text was found, but the clipboard never read back as that text.
    ClipboardUnchanged,
    Error,
    /// Asked while a capture was already open.
    Busy,
    /// Windows has no reader for the chosen language.
    NoEngine,
}

/// The wParam of a session's "done" message for a text capture. Image captures use 0, 1 and 2.
pub const DONE_TEXT_BASE: usize = 10;

impl TextOutcome {
    pub fn done_code(&self) -> usize {
        DONE_TEXT_BASE
            + match self {
                TextOutcome::Cancelled => 0,
                TextOutcome::Copied(_) => 1,
                TextOutcome::NoText => 2,
                TextOutcome::ClipboardUnchanged => 3,
                TextOutcome::Error => 4,
                TextOutcome::Busy => 5,
                TextOutcome::NoEngine => 6,
            }
    }

    pub fn name_of_code(code: usize) -> &'static str {
        match code.wrapping_sub(DONE_TEXT_BASE) {
            0 => "cancelled",
            1 => "copied",
            2 => "no-text",
            3 => "clipboard-unchanged",
            4 => "error",
            5 => "busy",
            6 => "no-engine",
            _ => "unknown",
        }
    }

    /// Failures also go to the notification centre, so they are not lost when the card is missed.
    pub fn code_is_failure(code: usize) -> bool {
        matches!(code.wrapping_sub(DONE_TEXT_BASE), 3 | 4 | 6)
    }
}

/// A release at or below this size on either side cancels instead of reading.
pub const MIN_SELECTION: i32 = 10;

pub fn selection_usable(width: i32, height: i32) -> bool {
    width > MIN_SELECTION && height > MIN_SELECTION
}

/// The recognized lines with the screen's line breaks kept: each trimmed, empty ones dropped.
pub fn join_lines<S: AsRef<str>>(lines: impl IntoIterator<Item = S>) -> String {
    lines.into_iter().map(|l| l.as_ref().trim().to_string()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\r\n")
}

/// The largest side the upscale may reach (WinTextCapture's MaxDimension).
pub const MAX_DIMENSION: u32 = 3200;

/// The integer upscale for a selection: 3 for a low strip (under 90 px), else 2, lowered until both
/// sides fit in `max_dimension`. Divides instead of multiplying, so huge sides cannot overflow.
pub fn scale_for(width: u32, height: u32, max_dimension: u32) -> u32 {
    if width == 0 || height == 0 || max_dimension == 0 {
        return 1;
    }
    let mut scale = if height < 90 { 3 } else { 2 };
    while scale > 1 && (width > max_dimension / scale || height > max_dimension / scale) {
        scale -= 1;
    }
    scale
}

/// The size of the image the reader gets. A side over the engine's limit shrinks both sides; else
/// the upscale above, bounded by 3200.
pub fn target_size(width: u32, height: u32, engine_max: u32) -> (u32, u32) {
    let engine_max = engine_max.max(1);
    let largest = width.max(height);
    if largest > engine_max {
        let shrink = |side: u32| ((side as u64 * engine_max as u64 / largest as u64) as u32).max(1);
        return (shrink(width), shrink(height));
    }
    let scale = scale_for(width, height, engine_max.min(MAX_DIMENSION));
    (width * scale, height * scale)
}

/// Length in UTF-16 code units, the unit WinTextCapture counted in.
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Whitespace collapsed to single spaces, cut on a whole user-perceived character to at most `max`
/// UTF-16 units, with "..." when cut. Emoji and combining marks are never split.
/// A line of the "all texts" menu: an ampersand would otherwise mark a shortcut letter.
pub fn menu_label(text: &str, when: &str) -> String {
    format!("{}\t{when}", preview(text, 60).replace('&', "&&"))
}

pub fn preview(text: &str, max: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if utf16_len(&joined) <= max {
        return joined;
    }
    let limit = max.saturating_sub(3);
    let (mut units, mut bytes) = (0usize, 0usize);
    for g in joined.graphemes(true) {
        let end = units + utf16_len(g);
        if end > limit {
            break;
        }
        units = end;
        bytes += g.len();
    }
    format!("{}...", &joined[..bytes])
}

/// Non-empty lines and user-perceived characters (line breaks not counted).
pub fn measure(text: &str) -> (usize, usize) {
    let lines: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches('\r')).filter(|l| !l.trim().is_empty()).collect();
    let chars = lines.iter().map(|l| l.graphemes(true).count()).sum();
    (lines.len(), chars)
}

pub const CLIP_ATTEMPTS: u32 = 6;
pub const CLIP_DELAY_MS: u32 = 35;

/// CRLF and lone CR become LF.
pub fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

pub fn same_text(a: &str, b: &str) -> bool {
    normalize_newlines(a) == normalize_newlines(b)
}

/// WinTextCapture's clipboard loop. Writes, then reads back. An error (another program holds the
/// clipboard, often a manager reading what was just written) waits and tries again. Reading back
/// something else means someone replaced it: write again. An error on the last try comes back as
/// it is; all tries different gives Ok(false).
pub fn set_text_with_retry<E>(text: &str, mut set: impl FnMut(&str) -> Result<(), E>, mut get: impl FnMut() -> Result<String, E>, mut delay: impl FnMut(u32), attempts: u32) -> Result<bool, E> {
    assert!(attempts > 0, "attempts must be at least 1");
    let mut written = false;
    for i in 0..attempts {
        let last = i + 1 == attempts;
        let step = (|| {
            if !written {
                set(text)?;
                written = true;
            }
            get()
        })();
        match step {
            Ok(back) if same_text(text, &back) => return Ok(true),
            Ok(_) => written = false,
            Err(e) if last => return Err(e),
            Err(_) => {}
        }
        if !last {
            delay(CLIP_DELAY_MS);
        }
    }
    Ok(false)
}

/// The bytes CF_UNICODETEXT holds: UTF-16LE with a terminating NUL.
pub fn unicode_text_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16().chain(Some(0)).flat_map(u16::to_le_bytes).collect()
}

/// The text in a CF_UNICODETEXT block: units up to the first NUL.
pub fn decode_unicode_text(units: &[u16]) -> String {
    let n = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..n])
}

/// Whitespace runs collapsed, the form WinTextCapture's measurements compared.
pub fn squeeze(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Edit distance over UTF-16 units (WinTextCapture's Lev compared C# chars).
pub fn levenshtein(a: &str, b: &str) -> usize {
    let (a, b): (Vec<u16>, Vec<u16>) = (a.encode_utf16().collect(), b.encode_utf16().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, x) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            cur[j + 1] = (cur[j] + 1).min(prev[j + 1] + 1).min(prev[j] + usize::from(x != y));
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Character error rate in percent against `truth`.
pub fn cer(read: &str, truth: &str) -> f64 {
    let t = squeeze(truth);
    100.0 * levenshtein(&squeeze(read), &t) as f64 / utf16_len(&t).max(1) as f64
}

/// The language the reader uses: "auto" follows the Windows profile, anything else is a tag.
pub fn is_auto_language(s: &str) -> bool {
    let t = s.trim();
    t.is_empty() || t.eq_ignore_ascii_case("auto")
}

/// A BCP-47 shape check for the config (Windows answers for what is really installed).
pub fn is_well_formed_tag(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty() && t.len() <= 35 && t.split('-').all(|p| !p.is_empty() && p.len() <= 8 && p.chars().all(|c| c.is_ascii_alphanumeric())) && t.split('-').next().is_some_and(|p| (2..=3).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphabetic()))
}

/// The engines to ask for, in order. Automatic: the profile's, then every installed language in
/// order. A chosen tag: only that one, so a missing language says so instead of reading in another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineRequest {
    Profile,
    Language(String),
}

pub fn engine_requests(selection: &str, installed: &[String]) -> Vec<EngineRequest> {
    if !is_auto_language(selection) {
        return vec![EngineRequest::Language(selection.trim().to_string())];
    }
    let mut tags: Vec<String> = installed.to_vec();
    tags.sort_by_key(|t| t.to_ascii_lowercase());
    tags.dedup_by_key(|t| t.to_ascii_lowercase());
    std::iter::once(EngineRequest::Profile).chain(tags.into_iter().map(EngineRequest::Language)).collect()
}

// ---- Automatic over more than one script ----
//
// A Windows reader reads one script. The Turkish one turns Chinese, Japanese or Korean into Latin
// garbage that is not empty (2026-11-13: the nick `EGO窥视#6240` came out as "EGOR#R\r\n#6240"), so
// "it read nothing, try the next" cannot catch it. In automatic mode the profile's reader reads
// first. When its reading gives a reason (`needs_second_look`), the installed readers of the other
// scripts read the same picture: every CJK reader (Han differs between them), one reader per other
// script (Cyrillic, Greek, Arabic). Where one of them saw letters of its own script, its words
// replace the first reader's words they cover; everything else stays as the first reader read it, so
// Turkish letters outside those spots are never touched. A reason that turns out wrong costs time
// only: the text changes only where another reader shows its own letters. A chosen language reads
// with that reader only, as before.

/// One recognised word and its box in the prepared picture, in pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct OcrWord {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// 0 for a word as its reader gave it; for a word another script's reader put in, the share of
    /// its letters that proved that script (`proof_share`, above 0). A weaker script's reader
    /// never replaces such a word.
    pub proof: f32,
}

impl OcrWord {
    pub fn new(text: &str, x: f32, y: f32, w: f32, h: f32) -> Self {
        OcrWord { text: text.to_string(), x, y, w, h, proof: 0.0 }
    }
    fn right(&self) -> f32 {
        self.x + self.w
    }
    fn bottom(&self) -> f32 {
        self.y + self.h
    }
    /// The same spot: they share at least a fifth of the narrower width and half of the lower height.
    fn overlaps(&self, o: &OcrWord) -> bool {
        let ix = self.right().min(o.right()) - self.x.max(o.x);
        let iy = self.bottom().min(o.bottom()) - self.y.max(o.y);
        ix > 0.2 * self.w.min(o.w).max(1.0) && iy > 0.5 * self.h.min(o.h).max(1.0)
    }
    /// On the same text line: they share half of the lower height.
    fn same_row(&self, top: f32, bottom: f32) -> bool {
        let iy = self.bottom().min(bottom) - self.y.max(top);
        iy > 0.5 * self.h.min(bottom - top).max(1.0)
    }
}

/// One line as a reader gave it: the line text Windows joined, and its words.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct OcrLine {
    pub text: String,
    pub words: Vec<OcrWord>,
}

/// What one reader read.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct EngineReading {
    pub tag: String,
    /// The reader's ISO 15924 script as Windows names it ("Latn", "Hans", "Jpan"...); may be empty.
    pub script: String,
    pub lines: Vec<OcrLine>,
}

impl EngineReading {
    pub fn text(&self) -> String {
        join_lines(self.lines.iter().map(|l| l.text.as_str()))
    }
}

/// A script family: readers of one family read the same letters, so at most one reading of each
/// family is put in. It comes from the ISO 15924 script Windows names for each reader
/// (`Language.Script`), or the tag's own script subtag; no list of languages is kept here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Family {
    Latin,
    /// Han, kana and Hangul: Hans, Hant, Jpan, Kore (they share Han).
    Han,
    Cyrillic,
    Greek,
    Arabic,
    /// Any other script, by its code (or by the language when Windows names none).
    Other(String),
}

fn primary_subtag(tag: &str) -> String {
    tag.trim().split('-').next().unwrap_or("").to_ascii_lowercase()
}

/// The reader's script code: what Windows says, else a four-letter subtag of the tag, else "".
pub fn reader_script(tag: &str, os_script: &str) -> String {
    let s = os_script.trim();
    if s.len() == 4 && s.chars().all(|c| c.is_ascii_alphabetic()) {
        return s.to_string();
    }
    tag.trim().split('-').skip(1).find(|p| p.len() == 4 && p.chars().all(|c| c.is_ascii_alphabetic())).map(str::to_string).unwrap_or_default()
}

pub fn family_of(tag: &str, script: &str) -> Family {
    match script.trim().to_ascii_lowercase().as_str() {
        "latn" => Family::Latin,
        "hans" | "hant" | "hani" | "jpan" | "hira" | "kana" | "kore" | "hang" => Family::Han,
        "cyrl" => Family::Cyrillic,
        "grek" => Family::Greek,
        "arab" => Family::Arabic,
        "" => Family::Other(primary_subtag(tag)),
        other => Family::Other(other.to_string()),
    }
}

fn is_han(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x3FFFF)
}

fn is_kana(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x309F | 0x30A0..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F)
}

fn is_hangul(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF)
}

/// A Han, kana or Hangul letter.
pub fn is_cjk_letter(c: char) -> bool {
    is_han(c) || is_kana(c) || is_hangul(c)
}

/// CJK letters and the CJK punctuation and full-width forms: written without spaces around them.
fn is_cjk_tight(c: char) -> bool {
    is_cjk_letter(c) || matches!(c as u32, 0x3000..=0x303F | 0xFF00..=0xFFEF)
}

pub fn cjk_letters(s: &str) -> usize {
    s.chars().filter(|&c| is_cjk_letter(c)).count()
}

fn is_latin_letter(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c as u32, 0x00C0..=0x024F | 0x1E00..=0x1EFF)
}

/// The family a letter belongs to, for the families a second reader can prove; None for Latin,
/// digits and punctuation.
fn family_of_char(c: char) -> Option<Family> {
    if is_cjk_letter(c) {
        return Some(Family::Han);
    }
    match c as u32 {
        0x0400..=0x052F => Some(Family::Cyrillic),
        0x0370..=0x03FF | 0x1F00..=0x1FFF => Some(Family::Greek),
        0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => Some(Family::Arabic),
        _ if c.is_alphabetic() && !is_latin_letter(c) => Some(Family::Other(String::new())),
        _ => None,
    }
}

/// Cyrillic and Greek letters a Latin letter or a digit looks like: a Cyrillic reader may read
/// "EGO" as "ЕГО" and "5523" as "552З".
fn latin_lookalike(c: char) -> bool {
    "АВЕКМНОРСТХУЗаеорсухзбіјѕІЈЅԁԛԝΑΒΕΖΗΙΚΜΝΟΡΤΥΧοικνυ".contains(c)
}

/// Letters in `word` that prove a reader of `family` saw its own script (0: none). A Cyrillic or
/// Greek word proves nothing unless at least two thirds of its letters look like no Latin letter
/// or digit (measured 2026-11-13: the Bulgarian reader turned "Oyuncu:" into "Оуипси:", half
/// lookalikes). A reader of any other script proves with letters that are neither Latin nor of the
/// families above.
pub fn proof_letters(word: &str, family: &Family) -> usize {
    if *family == Family::Han && !word.chars().any(|c| is_cjk_letter(c) && !stroke_like(c)) {
        return 0;
    }
    let mine: Vec<char> = word
        .chars()
        .filter(|&c| match (family_of_char(c), family) {
            (Some(Family::Other(_)), Family::Other(_)) => true,
            (Some(f), fam) => &f == fam,
            (None, _) => false,
        })
        .collect();
    match family {
        Family::Han | Family::Arabic | Family::Other(_) => mine.len(),
        Family::Cyrillic | Family::Greek => {
            let distinct = mine.iter().filter(|&&c| !latin_lookalike(c)).count();
            if distinct > 0 && 3 * distinct >= 2 * mine.len() { distinct } else { 0 }
        }
        Family::Latin => 0,
    }
}

/// Han letters that are one or two strokes and look like a dash, a bar, a box or a Latin letter: a
/// Chinese reader sees them in any script (measured 2026-11-13: "AB-1043" became "AB一1043" and an
/// Arabic nick "8841 # 一"). A word of only these proves nothing.
fn stroke_like(c: char) -> bool {
    "一丨丿乀乁乙乚亅丶亻冂冖匚匸囗口二十卜人入八工刂丁".contains(c)
}

/// The share of a word's letters that prove `family` (0 to 1).
pub fn proof_share(word: &str, family: &Family) -> f32 {
    let letters = word.chars().filter(|c| c.is_alphabetic()).count();
    if letters == 0 { 0.0 } else { proof_letters(word, family) as f32 / letters as f32 }
}

/// The readers that read the picture again after the first one: in automatic mode, every
/// installed reader (tag, script) whose family is neither Latin (the first reader reads Latin) nor
/// the first reader's own. Sorted by tag, each once.
pub fn second_readers(selection: &str, first: (&str, &str), installed: &[(String, String)]) -> Vec<String> {
    if !is_auto_language(selection) {
        return Vec::new();
    }
    let own = family_of(first.0, &reader_script(first.0, first.1));
    let mut tags: Vec<String> = installed
        .iter()
        .filter(|(t, s)| {
            let f = family_of(t, &reader_script(t, s));
            f != Family::Latin && f != own
        })
        .map(|(t, _)| t.trim().to_string())
        .collect();
    tags.sort_by_key(|t| t.to_ascii_lowercase());
    tags.dedup_by_key(|t| t.to_ascii_lowercase());
    tags
}

/// How strongly a reading proves its family, highest wins:
/// (own letters count, own letters, Chinese reader, agreement, proving letters, width, order).
///
/// Han is shared by the Chinese (Hans, Hant), Japanese and Korean readers; measured 2026-11-13 the
/// Korean reader turned the nick 窥视 into 麵帆 and the Japanese one into 窺祝. So:
/// - Kana decide for the Japanese reader and Hangul for the Korean one, but only when at least 2
///   and a quarter of its CJK letters (a stray kana in a Chinese line does not count).
/// - Otherwise a Chinese reader comes first, and among readers the one whose Han letters the other
///   Han readers also saw, on average, most often (agreement), then more proving letters, wider
///   proving words, then Hans, Hant, Jpan, Kore.
///
/// Other families: proving letters, then width.
type Strength = (bool, usize, bool, i64, usize, i64, i32);

fn strength(r: &EngineReading, others: &[EngineReading]) -> Strength {
    let script = reader_script(&r.tag, &r.script).to_ascii_lowercase();
    let family = family_of(&r.tag, &script);
    let words: Vec<&str> = r.lines.iter().flat_map(|l| l.text.split_whitespace()).collect();
    let proof: usize = words.iter().map(|w| proof_letters(w, &family)).sum();
    let width: f32 = r.lines.iter().flat_map(|l| &l.words).filter(|w| proof_letters(&w.text, &family) > 0).map(|w| w.w).sum();
    if family != Family::Han {
        return (false, 0, false, 0, proof, width as i64, 0);
    }
    let all = r.text();
    let own = match script.as_str() {
        "jpan" | "hira" | "kana" => all.chars().filter(|&c| is_kana(c)).count(),
        "kore" | "hang" => all.chars().filter(|&c| is_hangul(c)).count(),
        _ => 0,
    };
    let counts = own >= 2 && 4 * own >= cjk_letters(&all);
    let chinese = matches!(script.as_str(), "hans" | "hant" | "hani");
    let han_texts: Vec<String> = others.iter().filter(|o| !std::ptr::eq(*o, r) && family_of(&o.tag, &reader_script(&o.tag, &o.script)) == Family::Han).map(|o| o.text()).collect();
    let han: Vec<char> = all.chars().filter(|&c| is_han(c)).collect();
    let agree = if han.is_empty() { 0 } else { han.iter().map(|c| han_texts.iter().filter(|t| t.contains(*c)).count()).sum::<usize>() as i64 * 1000 / han.len() as i64 };
    let order = match script.as_str() {
        "hans" => 3,
        "hant" => 2,
        "jpan" => 1,
        _ => 0,
    };
    (counts, if counts { own } else { 0 }, chinese, agree, proof, width as i64, order)
}

/// Symbols that do not sit inside a real word: the Turkish reader's garbage for Han ("EGOR#R").
const ODD_INSIDE: &str = "#@$%^&*|\\<>~{}[]=_¦§";

fn odd_word(word: &str) -> bool {
    let c: Vec<char> = word.trim().chars().collect();
    let (Some(a), Some(b)) = (c.iter().position(|x| x.is_alphanumeric()), c.iter().rposition(|x| x.is_alphanumeric())) else {
        return c.iter().any(|x| ODD_INSIDE.contains(*x));
    };
    c[a..=b].iter().any(|x| ODD_INSIDE.contains(*x))
}

/// Ink the first reader left outside its words, over all the ink, that makes a reason by itself.
/// Measured 2026-11-13: the Turkish reader dropped "EGO窥视#6240" from a Turkish line and a whole
/// Chinese line under a Turkish one without a trace in its text; only the unread ink shows it.
pub const UNREAD_INK_REASON: f64 = 0.08;

/// Whether the first reading gives a reason to ask the readers of the other scripts: it is empty,
/// it left ink unread (`unread_ink` at or over `UNREAD_INK_REASON`), or it has a word with a symbol
/// inside. A reading with only clean words and all the ink read is taken as it is, Turkish or
/// plain ASCII: with every reader installed, asking them about clean English cost 300-330 ms on a
/// 417x69 crop (measured 2026-11-13), and every CJK, Cyrillic, Greek and Arabic fixture already
/// gave one of the reasons above.
pub fn needs_second_look(first: &EngineReading, unread_ink: f64) -> bool {
    let text = first.text();
    if text.trim().is_empty() || unread_ink >= UNREAD_INK_REASON {
        return true;
    }
    let words: Vec<&str> = first.lines.iter().flat_map(|l| if l.words.is_empty() { l.text.split_whitespace().collect::<Vec<_>>() } else { l.words.iter().map(|w| w.text.as_str()).collect() }).collect();
    words.iter().any(|w| odd_word(w))
}

/// The share of ink pixels (0..1) outside every word box, each box grown by a fifth of its height.
/// `px` is 32bpp BGRA, top-down. Ink: a luminance more than 64 away from the most common one (the
/// background). No ink: 0.
pub fn unread_ink(w: u32, h: u32, px: &[u8], words: &[&OcrWord]) -> f64 {
    let (w, h) = (w as usize, h as usize);
    if w == 0 || h == 0 || px.len() < w * h * 4 {
        return 0.0;
    }
    let lum = |p: &[u8; 4]| (p[2] as u32 * 299 + p[1] as u32 * 587 + p[0] as u32 * 114) / 1000;
    let mut hist = [0usize; 32];
    let pixels = &px.as_chunks::<4>().0[..w * h];
    for p in pixels {
        hist[(lum(p) / 8) as usize] += 1;
    }
    let bin = hist.iter().enumerate().max_by_key(|(_, n)| **n).map(|(i, _)| i).unwrap_or(0);
    let bg = bin as i32 * 8 + 4;
    let mut covered = vec![false; w * h];
    for b in words {
        let pad = 0.2 * b.h.max(0.0);
        let x0 = (b.x - pad).floor().max(0.0) as usize;
        let y0 = (b.y - pad).floor().max(0.0) as usize;
        let x1 = ((b.right() + pad).ceil().max(0.0) as usize).min(w);
        let y1 = ((b.bottom() + pad).ceil().max(0.0) as usize).min(h);
        for y in y0.min(h)..y1 {
            covered[y * w + x0.min(x1)..y * w + x1].fill(true);
        }
    }
    let (mut ink, mut unread) = (0usize, 0usize);
    for (i, p) in pixels.iter().enumerate() {
        if (lum(p) as i32 - bg).abs() > 64 {
            ink += 1;
            if !covered[i] {
                unread += 1;
            }
        }
    }
    if ink == 0 { 0.0 } else { unread as f64 / ink as f64 }
}

/// Words of one row in reading order; `true` marks a word the base reading had. Two base words
/// get a space, as the reader joined them. Next to a word another reader gave, the gap decides:
/// under a fifth of the row's height there is no space (CJK readers give each letter as its own
/// word, and the Chinese one splits "#6240" into "#" and "6240"). A row of mostly Arabic words
/// reads right to left.
fn join_words(words: &mut [(&OcrWord, bool)]) -> String {
    let arabic = words.iter().filter(|(w, _)| proof_letters(&w.text, &Family::Arabic) > 0).count();
    words.sort_by(|a, b| a.0.x.total_cmp(&b.0.x));
    if 2 * arabic > words.len() {
        words.reverse();
    }
    let mut out = String::new();
    for (i, (w, base)) in words.iter().enumerate() {
        if i > 0 {
            let (prev, prev_base) = words[i - 1];
            let (left, right) = if prev.x <= w.x { (prev, *w) } else { (*w, prev) };
            let gap = right.x - left.right();
            // Chinese and Japanese put no space between letters, however wide the reader's gap
            // (measured: "さ く ら" from the Japanese reader); Korean does, so Hangul keeps the gap rule.
            let unspaced = |c: char| is_han(c) || is_kana(c) || matches!(c as u32, 0x3000..=0x303F | 0xFF00..=0xFFEF);
            let both_unspaced = prev.text.trim().chars().last().is_some_and(unspaced) && w.text.trim().chars().next().is_some_and(unspaced);
            if !both_unspaced && ((prev_base && *base) || gap >= 0.2 * prev.h.max(w.h)) {
                out.push(' ');
            }
        }
        out.push_str(w.text.trim());
    }
    out
}

/// Spaces between two CJK letters dropped: for a reading that has no word boxes.
pub fn tidy_cjk_spaces(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &ch) in c.iter().enumerate() {
        if ch == ' ' {
            let before = c[..i].iter().rev().find(|x| **x != ' ');
            let after = c[i + 1..].iter().find(|x| **x != ' ');
            if before.is_some_and(|x| is_cjk_tight(*x)) && after.is_some_and(|x| is_cjk_tight(*x)) {
                continue;
            }
        }
        out.push(ch);
    }
    out
}

fn row_span<'a>(words: impl Iterator<Item = &'a OcrWord>) -> (f32, f32) {
    words.fold((f32::INFINITY, f32::NEG_INFINITY), |(t, b), w| (t.min(w.y), b.max(w.bottom())))
}

/// `base` with `other`'s words put in where they prove `family`. See `choose_reading`.
fn merge(base: &EngineReading, other: &EngineReading, family: &Family) -> EngineReading {
    let base_words: Vec<(usize, &OcrWord)> = base.lines.iter().enumerate().flat_map(|(li, l)| l.words.iter().map(move |w| (li, w))).collect();
    // A word proves when it has letters of the family and no word a stronger script's reader put
    // in sits under it (families go in strongest first; the Chinese reader sees Han strokes in
    // Greek and Cyrillic words).
    let proves = |w: &OcrWord| proof_letters(&w.text, family) > 0 && !base_words.iter().any(|(_, p)| p.proof > 0.0 && w.overlaps(p));
    // Base words sitting on a proving word are wrong.
    let wrong: Vec<&OcrWord> = base_words.iter().map(|(_, w)| *w).filter(|p| other.lines.iter().flat_map(|l| &l.words).any(|o| proves(o) && o.overlaps(p))).collect();
    let mut taken: Vec<OcrWord> = Vec::new();
    for line in other.lines.iter().filter(|l| l.words.iter().any(proves)) {
        for o in &line.words {
            let covers_wrong = wrong.iter().any(|p| o.overlaps(p));
            let alone = !base_words.iter().any(|(_, p)| o.overlaps(p));
            if proves(o) || covers_wrong || alone {
                taken.push(OcrWord { proof: proof_share(&o.text, family), ..o.clone() });
            }
        }
    }
    let kept: Vec<(usize, &OcrWord)> = base_words.iter().copied().filter(|(_, p)| !taken.iter().any(|o| o.overlaps(p))).collect();
    // A base line is touched when it lost a word or a taken word sits on its row.
    let touched: Vec<bool> = base
        .lines
        .iter()
        .enumerate()
        .map(|(li, l)| {
            let lost = l.words.len() != kept.iter().filter(|(k, _)| *k == li).count();
            let (top, bottom) = row_span(l.words.iter());
            lost || taken.iter().any(|o| o.same_row(top, bottom))
        })
        .collect();
    // (top, line) of each output line; an untouched line stays word for word.
    let mut out: Vec<(f32, OcrLine)> = Vec::new();
    for (li, l) in base.lines.iter().enumerate() {
        if !touched[li] && !l.text.trim().is_empty() {
            out.push((row_span(l.words.iter()).0, l.clone()));
        }
    }
    let mut pool: Vec<(&OcrWord, bool)> = kept.iter().filter(|(li, _)| touched[*li]).map(|(_, w)| (*w, true)).chain(taken.iter().map(|w| (w, false))).collect();
    pool.sort_by(|a, b| a.0.y.total_cmp(&b.0.y).then(a.0.x.total_cmp(&b.0.x)));
    // (top, bottom, words with their origin) of each row.
    type Row<'a> = (f32, f32, Vec<(&'a OcrWord, bool)>);
    let mut rows: Vec<Row> = Vec::new();
    for (w, base) in pool {
        match rows.iter_mut().find(|(t, b, _)| w.same_row(*t, *b)) {
            Some((t, b, v)) => {
                *t = t.min(w.y);
                *b = b.max(w.bottom());
                v.push((w, base));
            }
            None => rows.push((w.y, w.bottom(), vec![(w, base)])),
        }
    }
    for (top, _, mut v) in rows {
        let text = join_words(&mut v);
        out.push((top, OcrLine { text, words: v.into_iter().map(|(w, _)| w.clone()).collect() }));
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    EngineReading { tag: base.tag.clone(), script: base.script.clone(), lines: out.into_iter().map(|(_, l)| l).collect() }
}

/// Which text goes on the clipboard, and which second readers gave part of it (empty: the first
/// reader's text, unchanged to the byte).
///
/// - A second reading proves its family when it has a letter of it (Cyrillic and Greek: letters
///   that look like no Latin one). None does: the first reading as it is.
/// - Per family, the strongest reading is used (`strength`; a tie: the earlier one). Families are
///   put in from the most proving letters down.
/// - Its words are taken where they prove the family, where they cover a first-reader word that
///   sits on a proving word, and on its proving lines where the first reader saw nothing.
///   First-reader words under a taken word are dropped; the rest stay. A line nothing touched
///   stays word for word; a touched one is built again from the boxes, top to bottom, left to right
///   (right to left for an Arabic row).
pub fn choose_reading(first: &EngineReading, others: &[EngineReading]) -> (String, Vec<usize>) {
    // (family, index, strength) of the best reading of each family.
    type Best = (Family, usize, Strength);
    let mut best: Vec<Best> = Vec::new();
    for (i, r) in others.iter().enumerate() {
        let family = family_of(&r.tag, &reader_script(&r.tag, &r.script));
        let k = strength(r, others);
        if k.4 == 0 {
            continue;
        }
        match best.iter_mut().find(|(f, _, _)| *f == family) {
            Some(b) if b.2 >= k => {}
            Some(b) => *b = (family, i, k),
            None => best.push((family, i, k)),
        }
    }
    if best.is_empty() {
        return (first.text(), Vec::new());
    }
    best.sort_by(|a, b| b.2.4.cmp(&a.2.4).then(a.1.cmp(&b.1)));
    let mut current = first.clone();
    let mut used = Vec::new();
    for (family, i, _) in best {
        let other = &others[i];
        let has_boxes = other.lines.iter().any(|l| !l.words.is_empty());
        let base_has_text_without_boxes = current.lines.iter().all(|l| l.words.is_empty()) && current.lines.iter().any(|l| !l.text.trim().is_empty());
        used.push(i);
        if !has_boxes || base_has_text_without_boxes {
            // No boxes to place words with: that reading whole.
            let text = other.text();
            return (if family == Family::Han { tidy_cjk_spaces(&text) } else { text }, used);
        }
        let next = merge(&current, other, &family);
        if next.text() == current.text() {
            used.pop(); // it proved nothing that a stronger script had not already put in
        }
        current = next;
    }
    (current.text(), used)
}

/// The language menu: Automatic first, then "Name (tag)" sorted by name; a chosen language that is
/// no longer installed stays listed, so the box does not lie about what is set.
pub fn language_options(auto_label: &str, unavailable: impl Fn(&str) -> String, mut installed: Vec<(String, String)>, selection: &str) -> Vec<(String, String)> {
    installed.sort_by_key(|(_, name)| name.to_lowercase());
    let mut v = vec![("auto".to_string(), auto_label.to_string())];
    v.extend(installed.into_iter().map(|(tag, name)| {
        let label = format!("{name} ({tag})");
        (tag, label)
    }));
    if !is_auto_language(selection) && !v.iter().any(|(t, _)| t.eq_ignore_ascii_case(selection.trim())) {
        v.push((selection.trim().to_string(), unavailable(selection.trim())));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn done_codes_stay_apart_from_image_codes() {
        let all = [TextOutcome::Cancelled, TextOutcome::Copied("x".into()), TextOutcome::NoText, TextOutcome::ClipboardUnchanged, TextOutcome::Error, TextOutcome::Busy, TextOutcome::NoEngine];
        assert_eq!(all.len(), 7);
        for o in &all {
            assert!(o.done_code() >= DONE_TEXT_BASE, "{o:?}");
        }
        assert_eq!(TextOutcome::name_of_code(11), "copied");
        assert_eq!(TextOutcome::name_of_code(2), "unknown", "an image code");
        assert!(TextOutcome::code_is_failure(14) && !TextOutcome::code_is_failure(11));
    }

    #[test]
    fn selection_of_ten_pixels_or_less_is_cancelled() {
        assert!(!selection_usable(10, 200));
        assert!(!selection_usable(200, 10));
        assert!(!selection_usable(0, 0));
        assert!(selection_usable(11, 11));
    }

    #[test]
    fn line_breaks_of_the_screen_are_kept() {
        assert_eq!(join_lines([" Şirket ", "", "  ", "Görüşme 14:30"]), "Şirket\r\nGörüşme 14:30");
        assert_eq!(join_lines(Vec::<String>::new()), "");
    }

    #[test]
    fn upscale_rules_match_wintextcapture() {
        assert_eq!(scale_for(400, 60, MAX_DIMENSION), 3);
        assert_eq!(scale_for(900, 160, MAX_DIMENSION), 2);
        assert_eq!(scale_for(2500, 1400, MAX_DIMENSION), 1);
        assert_eq!(scale_for(100, 89, MAX_DIMENSION), 3);
        assert_eq!(scale_for(100, 90, MAX_DIMENSION), 2);
        assert_eq!(scale_for(1100, 40, MAX_DIMENSION), 2);
        assert_eq!(scale_for(u32::MAX, u32::MAX, MAX_DIMENSION), 1, "no overflow");
        assert_eq!(scale_for(0, 10, MAX_DIMENSION), 1);
    }

    #[test]
    fn sizes_stay_inside_the_engine_limit() {
        assert_eq!(target_size(400, 60, 10000), (1200, 180));
        assert_eq!(target_size(3300, 40, 10000), (3300, 40), "over the upscale cap, under the limit: left alone");
        assert_eq!(target_size(3300, 12, 3200), (3200, 11));
        assert_eq!(target_size(12, 3300, 3200), (11, 3200));
        assert_eq!(target_size(60, 20, 32), (32, 10));
        for side in [9999, 10000, 10001, 20000, u32::MAX] {
            for other in [1, 40, 89, 90, 10000] {
                for (w, h) in [(side, other), (other, side)] {
                    let (a, b) = target_size(w, h, 10000);
                    assert!(a <= 10000 && b <= 10000, "{w}x{h} -> {a}x{b}");
                }
            }
        }
    }

    #[test]
    fn menu_label_shows_the_ampersand_and_stays_short() {
        assert_eq!(menu_label("Ar & Ge\r\nraporu", "12:00"), "Ar && Ge raporu\t12:00");
        assert!(menu_label(&"a".repeat(500), "x").chars().count() < 70);
    }

    #[test]
    fn preview_collapses_whitespace_and_keeps_whole_characters() {
        assert_eq!(preview("hello\r\n  world", 80), "hello world");
        for tail in ["😀", "e\u{301}", "👩‍💻"] {
            let long = format!("{}{tail}suffix", "a".repeat(76));
            assert_eq!(preview(&long, 80), format!("{}...", "a".repeat(76)), "{tail}");
        }
    }

    #[test]
    fn measure_counts_lines_and_characters() {
        assert_eq!(measure("ab\r\n\r\nç😀\n"), (2, 4));
        assert_eq!(measure(""), (0, 0));
        assert_eq!(measure("   "), (0, 0));
    }

    #[test]
    fn clipboard_retry_survives_a_transient_lock() {
        let (sets, mut delays) = (std::cell::Cell::new(0), Vec::new());
        let r = set_text_with_retry(
            "text",
            |_| {
                sets.set(sets.get() + 1);
                if sets.get() < 3 { Err("locked") } else { Ok(()) }
            },
            || Ok("text".to_string()),
            |ms| delays.push(ms),
            6,
        );
        assert_eq!(r, Ok(true));
        assert_eq!((sets.get(), delays), (3, vec![CLIP_DELAY_MS; 2]));
    }

    #[test]
    fn clipboard_retry_gives_the_last_error_when_it_stays_locked() {
        let (mut sets, mut delays) = (0, Vec::new());
        let r = set_text_with_retry(
            "text",
            |_| {
                sets += 1;
                Err("locked")
            },
            || Ok(String::new()),
            |ms| delays.push(ms),
            3,
        );
        assert_eq!(r, Err("locked"));
        assert_eq!((sets, delays.len()), (3, 2));
    }

    #[test]
    fn clipboard_retry_reports_an_unchanged_clipboard() {
        let (mut sets, mut delays) = (0, Vec::new());
        let r = set_text_with_retry(
            "new",
            |_| {
                sets += 1;
                Ok::<(), &str>(())
            },
            || Ok("old".to_string()),
            |ms| delays.push(ms),
            3,
        );
        assert_eq!(r, Ok(false));
        assert_eq!((sets, delays.len()), (3, 2), "someone replaced it: written again each time");
    }

    #[test]
    fn a_read_error_does_not_write_twice() {
        let (mut sets, reads) = (0, std::cell::Cell::new(0));
        let r = set_text_with_retry(
            "a",
            |_| {
                sets += 1;
                Ok::<(), &str>(())
            },
            || {
                reads.set(reads.get() + 1);
                if reads.get() == 1 { Err("locked") } else { Ok("a".to_string()) }
            },
            |_| {},
            6,
        );
        assert_eq!(r, Ok(true));
        assert_eq!(sets, 1);
    }

    #[test]
    fn newlines_compare_normalised_and_utf16_has_a_nul() {
        assert!(same_text("a\r\nb\rc", "a\nb\nc"));
        assert!(!same_text("a", "b"));
        assert_eq!(unicode_text_bytes("ı"), vec![0x31, 0x01, 0, 0]);
        assert_eq!(decode_unicode_text(&[0x131, 0, 0x41]), "ı");
    }

    #[test]
    fn error_rate_is_edit_distance_over_truth() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(cer("abc  d", "abc d"), 0.0);
        assert_eq!(cer("abcd", "abce"), 25.0);
    }

    #[test]
    fn automatic_asks_the_profile_then_installed_and_a_tag_asks_only_itself() {
        let inst = vec!["tr".to_string(), "en-US".to_string(), "EN-us".to_string()];
        assert_eq!(engine_requests("auto", &inst), vec![EngineRequest::Profile, EngineRequest::Language("en-US".into()), EngineRequest::Language("tr".into())]);
        assert_eq!(engine_requests("", &[]), vec![EngineRequest::Profile]);
        assert_eq!(engine_requests(" de-DE ", &inst), vec![EngineRequest::Language("de-DE".into())]);
    }

    #[test]
    fn language_menu_starts_with_automatic_and_keeps_a_missing_choice() {
        let unavailable = |t: &str| format!("Yok ({t})");
        let v = language_options("Otomatik", unavailable, vec![("tr".into(), "Türkçe".into()), ("de".into(), "Almanca".into())], "auto");
        assert_eq!(v, vec![("auto".to_string(), "Otomatik".to_string()), ("de".to_string(), "Almanca (de)".to_string()), ("tr".to_string(), "Türkçe (tr)".to_string())]);
        let w = language_options("Otomatik", unavailable, vec![], "fr-FR");
        assert_eq!(w[1], ("fr-FR".to_string(), "Yok (fr-FR)".to_string()));
    }

    fn line(words: &[(&str, f32, f32, f32, f32)]) -> OcrLine {
        let words: Vec<OcrWord> = words.iter().map(|&(t, x, y, w, h)| OcrWord::new(t, x, y, w, h)).collect();
        OcrLine { text: words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "), words }
    }

    /// The script Windows names for each tag used in these tests.
    fn os_script(tag: &str) -> &'static str {
        match tag.split('-').next().unwrap_or("") {
            "zh" if tag.contains("Hant") || tag.ends_with("TW") || tag.ends_with("HK") => "Hant",
            "zh" => "Hans",
            "ja" => "Jpan",
            "ko" => "Kore",
            "ru" | "bg" | "uk" => "Cyrl",
            "sr" if tag.contains("Cyrl") => "Cyrl",
            "el" => "Grek",
            "ar" => "Arab",
            "he" => "Hebr",
            _ => "Latn",
        }
    }

    fn reading(tag: &str, lines: Vec<OcrLine>) -> EngineReading {
        EngineReading { tag: tag.into(), script: os_script(tag).into(), lines }
    }

    /// The nick of 2026-11-13 (149x44 crop, upscaled 3x): what the Turkish reader gave, and a
    /// Chinese reading of the same boxes as Windows' CJK readers give it, one word per Han letter.
    fn nick() -> (EngineReading, EngineReading) {
        let tr = reading("tr", vec![line(&[("EGOR#R", 15.0, 40.0, 190.0, 50.0)]), line(&[("#6240", 214.0, 44.0, 150.0, 42.0)])]);
        let zh = reading("zh-Hans-CN", vec![line(&[("EGO", 15.0, 42.0, 85.0, 46.0), ("窥", 104.0, 38.0, 52.0, 54.0), ("视", 158.0, 38.0, 52.0, 54.0), ("#6240", 214.0, 44.0, 150.0, 42.0)])]);
        (tr, zh)
    }

    #[test]
    fn the_nick_reads_as_the_chinese_reader_saw_it() {
        let (tr, zh) = nick();
        assert_eq!(tr.text(), "EGOR#R\r\n#6240", "the clipboard of 2026-11-13");
        assert_eq!(choose_reading(&tr, &[zh]), ("EGO窥视#6240".to_string(), vec![0]));
    }

    #[test]
    fn turkish_text_is_left_to_the_first_reader() {
        let tr = reading("tr", vec![line(&[("Şirket", 10.0, 10.0, 120.0, 30.0), ("çalışanları", 140.0, 10.0, 200.0, 30.0)]), line(&[("Görüşme", 10.0, 60.0, 150.0, 30.0), ("İstanbul'da", 170.0, 60.0, 210.0, 30.0)])]);
        let zh = reading("zh-Hans-CN", vec![line(&[("Sirket", 10.0, 10.0, 120.0, 30.0), ("calisanlari", 140.0, 10.0, 200.0, 30.0)]), line(&[("Gorusme", 10.0, 60.0, 150.0, 30.0), ("Istanbul'da", 170.0, 60.0, 210.0, 30.0)])]);
        assert_eq!(choose_reading(&tr, std::slice::from_ref(&zh)), ("Şirket çalışanları\r\nGörüşme İstanbul'da".to_string(), vec![]));
        // Plain ASCII: the same, to the byte.
        let en = reading("tr", vec![line(&[("Error", 0.0, 0.0, 80.0, 30.0), ("0x80070643:", 90.0, 0.0, 170.0, 30.0)])]);
        assert_eq!(choose_reading(&en, &[zh]), ("Error 0x80070643:".to_string(), vec![]));
        // No second reader installed.
        let (tr, _) = nick();
        assert_eq!(choose_reading(&tr, &[]), ("EGOR#R\r\n#6240".to_string(), vec![]));
    }

    #[test]
    fn turkish_and_chinese_on_one_line_keep_the_turkish_letters() {
        // "Şirket 窥视 Görüşme": the Turkish reader turned the Han letters into "R#R".
        let tr = reading("tr", vec![line(&[("Şirket", 10.0, 10.0, 120.0, 40.0), ("R#R", 150.0, 8.0, 100.0, 44.0), ("Görüşme", 280.0, 10.0, 150.0, 40.0)])]);
        let zh = reading("zh-Hans-CN", vec![line(&[("Sirket", 10.0, 10.0, 120.0, 40.0), ("窥", 150.0, 8.0, 48.0, 44.0), ("视", 200.0, 8.0, 48.0, 44.0), ("Gorusme", 280.0, 10.0, 150.0, 40.0)])]);
        assert_eq!(choose_reading(&tr, &[zh]).0, "Şirket 窥视 Görüşme");
    }

    #[test]
    fn a_chinese_line_under_turkish_lines_changes_only_that_line() {
        let tr = reading("tr", vec![line(&[("Görüşme", 10.0, 10.0, 150.0, 30.0), ("14:30", 170.0, 10.0, 80.0, 30.0)]), line(&[("RRR", 10.0, 60.0, 100.0, 34.0)])]);
        let zh = reading("zh-Hans-CN", vec![line(&[("Gorusme", 10.0, 10.0, 150.0, 30.0), ("14:30", 170.0, 10.0, 80.0, 30.0)]), line(&[("你", 10.0, 60.0, 32.0, 34.0), ("好", 44.0, 60.0, 32.0, 34.0), ("吗", 78.0, 60.0, 32.0, 34.0)])]);
        assert_eq!(choose_reading(&tr, &[zh]), ("Görüşme 14:30\r\n你好吗".to_string(), vec![0]));
    }

    #[test]
    fn the_reader_with_more_cjk_letters_wins_and_a_tie_goes_to_the_first() {
        let tr = reading("tr", vec![line(&[("RR", 0.0, 0.0, 100.0, 40.0)])]);
        let ja = reading("ja", vec![line(&[("こ", 0.0, 0.0, 30.0, 40.0), ("ん", 32.0, 0.0, 30.0, 40.0), ("に", 64.0, 0.0, 30.0, 40.0)])]);
        let zh = reading("zh-Hans-CN", vec![line(&[("丙", 0.0, 0.0, 30.0, 40.0)])]);
        assert_eq!(choose_reading(&tr, &[zh.clone(), ja.clone()]), ("こんに".to_string(), vec![1]));
        assert_eq!(choose_reading(&tr, &[zh.clone(), zh.clone()]).1, vec![0]);
    }

    #[test]
    fn a_blank_first_reading_takes_the_cjk_line_whole() {
        let (_, zh) = nick();
        assert_eq!(choose_reading(&reading("tr", vec![]), &[zh]).0, "EGO窥视#6240");
    }

    #[test]
    fn without_boxes_the_cjk_reading_is_taken_with_its_spaces_closed() {
        let tr = reading("tr", vec![OcrLine { text: "EGOR#R".into(), words: vec![] }]);
        let zh = reading("zh-Hans-CN", vec![OcrLine { text: "EGO 窥 视 #6240".into(), words: vec![] }]);
        assert_eq!(choose_reading(&tr, &[zh]), ("EGO 窥视 #6240".to_string(), vec![0]));
        assert_eq!(tidy_cjk_spaces("a b 你 好 c"), "a b 你好 c");
    }

    #[test]
    fn korean_words_keep_their_spaces() {
        let tr = reading("tr", vec![line(&[("RRR", 0.0, 0.0, 300.0, 40.0)])]);
        let ko = reading("ko", vec![line(&[("안녕", 0.0, 0.0, 80.0, 40.0), ("하세요", 110.0, 0.0, 120.0, 40.0)])]);
        assert_eq!(choose_reading(&tr, &[ko]).0, "안녕 하세요");
    }

    #[test]
    fn only_automatic_asks_every_other_script_again() {
        let inst: Vec<(String, String)> = ["tr", "zh-Hans-CN", "zh-Hant-TW", "ja", "en-US", "ZH-hans-cn", "ko", "bg-BG", "ru", "sr-Cyrl-RS", "el", "ar", "sr-Latn-RS", "he"].iter().map(|t| (t.to_string(), os_script(t).to_string())).collect();
        let tr = ("tr", "Latn");
        assert_eq!(second_readers("auto", tr, &inst), ["ar", "bg-BG", "el", "he", "ja", "ko", "ru", "sr-Cyrl-RS", "zh-Hans-CN", "zh-Hant-TW"].map(String::from).to_vec(), "every reader of another script, no Latin one");
        assert_eq!(second_readers("", tr, &inst).len(), 10);
        assert!(second_readers("tr", tr, &inst).is_empty(), "a chosen language reads alone");
        assert!(second_readers("zh-Hans-CN", ("zh-Hans-CN", "Hans"), &inst).is_empty());
        assert_eq!(second_readers("auto", ("ja", "Jpan"), &inst), ["ar", "bg-BG", "el", "he", "ru", "sr-Cyrl-RS"].map(String::from).to_vec(), "a Han-family first reader is not asked about Han");
        assert!(second_readers("auto", tr, &[("tr".into(), "Latn".into()), ("en-US".into(), "Latn".into())]).is_empty());
        // Windows names no script: the tag's subtag, else a family of its own.
        assert_eq!(second_readers("auto", ("tr", ""), &[("tr".into(), "".into()), ("zh-Hant-TW".into(), "".into()), ("sr-Cyrl-RS".into(), "".into())]), vec!["sr-Cyrl-RS".to_string(), "zh-Hant-TW".into()]);
        assert_eq!(family_of("xx", ""), Family::Other("xx".into()));
        assert_eq!(family_of("am", "Ethi"), Family::Other("ethi".into()));
        assert!(is_cjk_letter('窥') && is_cjk_letter('カ') && is_cjk_letter('한') && !is_cjk_letter('ş') && !is_cjk_letter('İ'));
    }

    #[test]
    fn han_goes_to_the_chinese_reading_the_others_agree_with() {
        // Measured 2026-11-13 on the nick fixture with every reader installed.
        let (tr, _) = nick();
        fn w(t: &str, x: f32, wd: f32) -> (&str, f32, f32, f32, f32) {
            (t, x, 40.0, wd, 50.0)
        }
        let hans = reading("zh-Hans-CN", vec![line(&[w("EGO", 15.0, 85.0), w("窥", 104.0, 52.0), w("视", 158.0, 52.0)]), line(&[w("#", 214.0, 30.0), w("62", 246.0, 50.0), w("0", 300.0, 30.0)])]);
        let hant = reading("zh-Hant-HK", vec![line(&[w("EGO", 15.0, 85.0), w("窥", 104.0, 52.0), w("视", 158.0, 52.0)]), line(&[w("#", 214.0, 30.0), w("6240", 246.0, 120.0)])]);
        let ja = reading("ja", vec![line(&[w("EGO", 15.0, 85.0), w("窺", 104.0, 52.0), w("祝", 158.0, 52.0)]), line(&[w("#", 214.0, 30.0), w("6240", 246.0, 120.0)])]);
        let ko = reading("ko", vec![line(&[w("EGO麵帆#6240", 15.0, 350.0)])]);
        let (text, used) = choose_reading(&tr, &[ja, ko, hans, hant]);
        assert_eq!(text, "EGO窥视#6240");
        assert_eq!(used, vec![2], "zh-Hans: agrees with zh-Hant, first in order");
        // A pure-Han line read right only by zh-Hant: the others agree more with it.
        let tr2 = reading("tr", vec![line(&[("RRRR", 0.0, 0.0, 200.0, 40.0)])]);
        fn c(t: &str, x: f32) -> (&str, f32, f32, f32, f32) {
            (t, x, 0.0, 30.0, 40.0)
        }
        let hans2 = reading("zh-Hans-CN", vec![line(&[c("亻", 0.0), c("尔", 32.0), c("好", 64.0), c("世", 96.0), c("界", 128.0)])]);
        let hant2 = reading("zh-Hant-HK", vec![line(&[c("你", 0.0), c("好", 64.0), c("世", 96.0), c("界", 128.0)])]);
        let ja2 = reading("ja", vec![line(&[c("イ", 0.0), c("尓", 32.0), c("好", 64.0), c("世", 96.0), c("界", 128.0)])]);
        assert_eq!(choose_reading(&tr2, &[hans2, hant2, ja2]).0, "你好世界");
    }

    #[test]
    fn a_dash_read_as_han_proves_nothing() {
        let tr = reading("tr", vec![line(&[("(ref", 0.0, 0.0, 60.0, 40.0), ("AB-1043).", 70.0, 0.0, 160.0, 40.0)])]);
        let hk = reading("zh-Hant-HK", vec![line(&[("(", 0.0, 0.0, 10.0, 40.0), ("「", 12.0, 0.0, 10.0, 40.0), ("efAB", 24.0, 0.0, 80.0, 40.0), ("一", 106.0, 0.0, 20.0, 40.0), ("1043", 128.0, 0.0, 80.0, 40.0), (").", 210.0, 0.0, 20.0, 40.0)])]);
        assert_eq!(choose_reading(&tr, &[hk]), ("(ref AB-1043).".to_string(), vec![]));
        assert_eq!(proof_letters("一", &Family::Han), 0);
        assert_eq!(proof_letters("口窥", &Family::Han), 2, "a real Han letter makes the word count, strokes included");
    }

    #[test]
    fn kana_and_han_join_without_spaces_whatever_the_gap() {
        let tr = reading("tr", vec![line(&[("Player:", 0.0, 0.0, 120.0, 40.0), ("Lv.30", 500.0, 0.0, 80.0, 40.0)])]);
        let ja = reading("ja", vec![line(&[("Player:", 0.0, 0.0, 120.0, 40.0), ("さ", 140.0, 0.0, 30.0, 40.0), ("く", 185.0, 0.0, 25.0, 40.0), ("ら", 225.0, 0.0, 30.0, 40.0), ("東", 262.0, 0.0, 40.0, 40.0), ("京", 304.0, 0.0, 40.0, 40.0), ("#", 347.0, 0.0, 20.0, 40.0), ("4512", 369.0, 0.0, 100.0, 40.0), ("Lv.", 500.0, 0.0, 40.0, 40.0), ("30", 548.0, 0.0, 32.0, 40.0)])]);
        assert_eq!(choose_reading(&tr, &[ja]).0, "Player: さくら東京#4512 Lv.30");
    }

    #[test]
    fn han_goes_to_japanese_only_with_real_kana() {
        let tr = reading("tr", vec![line(&[("RRRR", 0.0, 0.0, 200.0, 40.0)])]);
        fn w(t: &str, x: f32) -> (&str, f32, f32, f32, f32) {
            (t, x, 0.0, 30.0, 40.0)
        }
        // Japanese text: the Japanese reading has kana and wins although Chinese read more Han.
        let ja = reading("ja", vec![line(&[w("東", 0.0), w("京", 32.0), w("へ", 64.0), w("ようこそ", 96.0)])]);
        let zh = reading("zh-Hans-CN", vec![line(&[w("东", 0.0), w("京", 32.0), w("个", 64.0), w("太", 96.0)])]);
        assert_eq!(choose_reading(&tr, &[ja.clone(), zh.clone()]), ("東京へようこそ".to_string(), vec![0]));
        // Chinese text: one stray kana does not make it Japanese; more Han wins.
        let ja = reading("ja", vec![line(&[w("你", 0.0), w("ノ", 32.0), w("世", 64.0), w("界", 96.0)])]);
        let zh = reading("zh-Hans-CN", vec![line(&[w("你", 0.0), w("好", 32.0), w("世", 64.0), w("界", 96.0)])]);
        assert_eq!(choose_reading(&tr, &[ja, zh.clone()]), ("你好世界".to_string(), vec![1]));
        // Pure Han read the same by both Chinese readers: Hans before Hant.
        let hant = reading("zh-Hant-TW", zh.lines.clone());
        assert_eq!(choose_reading(&tr, &[hant, zh]).1, vec![1]);
        // Korean: Hangul is its own letters.
        let ko = reading("ko", vec![line(&[w("한국어", 0.0)])]);
        let zh = reading("zh-Hans-CN", vec![line(&[w("韩", 0.0)])]);
        assert_eq!(choose_reading(&tr, &[ko, zh]).1, vec![0]);
    }

    #[test]
    fn a_weaker_script_does_not_overwrite_a_stronger_one() {
        // Measured 2026-11-13 on "Player: Γιώργος#5523 Lv.30": the Greek reader read it, the
        // Chinese one saw a Han stroke in it ("「[山pyoq"), the Bulgarian one "ГИБруос#552З".
        let tr = reading("tr", vec![line(&[("Player:", 0.0, 0.0, 120.0, 40.0), ("rubpyoç#5523", 130.0, 0.0, 200.0, 40.0), ("Lv.30", 350.0, 0.0, 80.0, 40.0)])]);
        let el = reading("el", vec![line(&[("Player:", 0.0, 0.0, 120.0, 40.0), ("Γιώργος#5523", 130.0, 0.0, 200.0, 40.0), ("Lv.30", 350.0, 0.0, 80.0, 40.0)])]);
        let bg = reading("bg", vec![line(&[("Player:", 0.0, 0.0, 120.0, 40.0), ("ГИБруос#552З", 130.0, 0.0, 200.0, 40.0), ("Lv.30", 350.0, 0.0, 80.0, 40.0)])]);
        let zh = reading("zh-Hans-CN", vec![line(&[("Player:", 0.0, 0.0, 120.0, 40.0), ("「[", 130.0, 0.0, 30.0, 40.0), ("山", 162.0, 0.0, 30.0, 40.0), ("pyoq#5523", 194.0, 0.0, 136.0, 40.0), ("Lv.30", 350.0, 0.0, 80.0, 40.0)])]);
        assert_eq!(choose_reading(&tr, &[bg, el, zh]).0, "Player: Γιώργος#5523 Lv.30");
        // Half-lookalike Cyrillic over a clean Latin word proves nothing.
        assert_eq!(proof_letters("Оуипси:", &Family::Cyrillic), 0);
        assert_eq!(proof_letters("оп", &Family::Cyrillic), 0);
        assert!(proof_letters("Дмитрий_Волков", &Family::Cyrillic) > 0);
    }

    #[test]
    fn greek_and_an_unknown_script_prove_with_their_own_letters() {
        let tr = reading("tr", vec![line(&[("Ka1nuepa", 0.0, 0.0, 160.0, 40.0), ("2026", 180.0, 0.0, 80.0, 40.0)])]);
        let el = reading("el", vec![line(&[("Καλημέρα", 0.0, 0.0, 160.0, 40.0), ("2026", 180.0, 0.0, 80.0, 40.0)])]);
        assert_eq!(choose_reading(&tr, &[el]), ("Καλημέρα 2026".to_string(), vec![0]));
        let lookalike = reading("el", vec![line(&[("ΚΑΤΟ", 0.0, 0.0, 160.0, 40.0)])]);
        assert_eq!(choose_reading(&reading("tr", vec![line(&[("KATO", 0.0, 0.0, 160.0, 40.0)])]), &[lookalike]).1, Vec::<usize>::new());
        let he = reading("he", vec![line(&[("שלום", 0.0, 0.0, 160.0, 40.0)])]);
        assert_eq!(choose_reading(&reading("tr", vec![line(&[("oiwu", 0.0, 0.0, 160.0, 40.0)])]), &[he]).0, "שלום");
    }

    #[test]
    fn a_second_look_needs_a_reason() {
        let clean_tr = reading("tr", vec![line(&[("Şirket", 0.0, 0.0, 100.0, 30.0), ("14:30'da", 110.0, 0.0, 100.0, 30.0)])]);
        assert!(!needs_second_look(&clean_tr, 0.0), "Turkish letters, clean words, all ink read: taken as it is");
        assert!(needs_second_look(&clean_tr, UNREAD_INK_REASON), "ink left unread");
        let (tr, _) = nick();
        assert!(needs_second_look(&tr, 0.0), "a symbol inside a word");
        assert!(!needs_second_look(&reading("tr", vec![line(&[("Error", 0.0, 0.0, 80.0, 30.0)])]), 0.0), "clean ASCII, all ink read: taken as it is");
        assert!(needs_second_look(&reading("tr", vec![line(&[("Error", 0.0, 0.0, 80.0, 30.0)])]), 0.2), "but not with ink left unread");
        assert!(needs_second_look(&reading("tr", vec![]), 0.0), "nothing read");
        let odd_tr = reading("tr", vec![line(&[("Görüşme", 0.0, 0.0, 100.0, 30.0), ("R#R", 110.0, 0.0, 60.0, 30.0)])]);
        assert!(needs_second_look(&odd_tr, 0.0));
        for ok in ["#6240", "C++", "AB-1043", "(ref", "1.234,56", "a/b"] {
            assert!(!odd_word(ok), "{ok}");
        }
    }

    #[test]
    fn unread_ink_counts_what_no_word_box_covers() {
        // 100x20 white, two black blocks: one under a word box, one not.
        let (w, h) = (100u32, 20u32);
        let mut px = vec![255u8; (w * h * 4) as usize];
        for y in 5..15 {
            for x in (10..30).chain(60..80) {
                let i = (y * w as usize + x) * 4;
                px[i..i + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
        let read = OcrWord::new("a", 10.0, 5.0, 20.0, 10.0);
        assert!((unread_ink(w, h, &px, &[&read]) - 0.5).abs() < 1e-9);
        let both = OcrWord::new("b", 60.0, 5.0, 20.0, 10.0);
        assert_eq!(unread_ink(w, h, &px, &[&read, &both]), 0.0);
        assert_eq!(unread_ink(w, h, &vec![255u8; (w * h * 4) as usize], &[]), 0.0, "no ink");
        assert_eq!(unread_ink(w, h, &px[..10], &[]), 0.0, "short buffer");
        let off = OcrWord::new("c", -50.0, -50.0, 400.0, 400.0);
        assert_eq!(unread_ink(w, h, &px, &[&off]), 0.0, "a box past the edges");
    }

    #[test]
    fn a_dropped_nick_comes_back_between_turkish_words() {
        // Measured 2026-11-13 on "Oyuncu: EGO窥视#6240 Görüşme 14:30": the Turkish reader dropped the
        // nick; the Chinese one read it, split "#" from "6240" and lost the Turkish letters.
        let tr = reading("tr", vec![line(&[("Oyuncu:", 30.0, 30.0, 170.0, 45.0), ("Görüşme", 640.0, 30.0, 220.0, 45.0), ("14:30", 880.0, 30.0, 120.0, 45.0)])]);
        let zh = reading("zh-Hans-CN", vec![line(&[("Oyuncu:", 30.0, 30.0, 170.0, 45.0), ("EGO", 215.0, 30.0, 90.0, 45.0), ("窥", 308.0, 26.0, 50.0, 52.0), ("视", 360.0, 26.0, 50.0, 52.0), ("#", 413.0, 30.0, 30.0, 45.0), ("6240", 446.0, 30.0, 110.0, 45.0), ("Görüsme", 640.0, 30.0, 220.0, 45.0), ("14", 880.0, 30.0, 45.0, 45.0), ("：", 928.0, 30.0, 20.0, 45.0), ("30", 952.0, 30.0, 45.0, 45.0)])]);
        assert_eq!(choose_reading(&tr, &[zh]), ("Oyuncu: EGO窥视#6240 Görüşme 14:30".to_string(), vec![0]));
    }

    #[test]
    fn cyrillic_that_looks_latin_proves_nothing() {
        let tr = reading("tr", vec![line(&[("EGO", 0.0, 0.0, 90.0, 40.0), ("TOP", 100.0, 0.0, 90.0, 40.0)])]);
        let ru = reading("ru", vec![line(&[("ЕГО", 0.0, 0.0, 90.0, 40.0), ("ТОР", 100.0, 0.0, 90.0, 40.0)])]);
        assert_eq!(choose_reading(&tr, &[ru]), ("EGO TOP".to_string(), vec![]));
        let tr = reading("tr", vec![line(&[("Ilpnbet", 0.0, 0.0, 140.0, 40.0), ("dostum", 150.0, 0.0, 120.0, 40.0)])]);
        let ru = reading("ru", vec![line(&[("Привет", 0.0, 0.0, 140.0, 40.0), ("dостум", 150.0, 0.0, 120.0, 40.0)])]);
        assert_eq!(choose_reading(&tr, &[ru]), ("Привет dostum".to_string(), vec![0]), "only the word that proves Cyrillic is taken");
    }

    #[test]
    fn an_arabic_row_reads_right_to_left_and_scripts_add_up() {
        let tr = reading("tr", vec![line(&[("lw", 0.0, 0.0, 60.0, 40.0), ("pjl", 70.0, 0.0, 60.0, 40.0)]), line(&[("RR", 0.0, 60.0, 60.0, 40.0)])]);
        let ar = reading("ar", vec![line(&[("مرحبا", 70.0, 0.0, 60.0, 40.0), ("بكم", 0.0, 0.0, 60.0, 40.0)])]);
        let zh = reading("zh-Hans-CN", vec![line(&[("你", 0.0, 60.0, 28.0, 40.0), ("好", 30.0, 60.0, 28.0, 40.0)])]);
        let (text, used) = choose_reading(&tr, &[zh, ar]);
        assert_eq!(text, "مرحبا بكم\r\n你好");
        assert_eq!(used, vec![1, 0], "more proving letters first");
    }

    #[test]
    fn language_tags_are_checked_for_shape() {
        for ok in ["tr", "tr-TR", "en-US", "zh-Hans-CN"] {
            assert!(is_well_formed_tag(ok), "{ok}");
        }
        for bad in ["", "not a tag", "x", "tr_TR", "-tr", "1a"] {
            assert!(!is_well_formed_tag(bad), "{bad}");
        }
    }
}
