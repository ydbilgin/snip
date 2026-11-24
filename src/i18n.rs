//! UI strings in English and Turkish. Every string has both; a test checks none is empty and that
//! the Turkish ones use real Turkish letters (ı, ş, ğ, ü, ö, ç), not ASCII stand-ins.

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum LangChoice {
    #[default]
    System,
    En,
    Tr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Tr,
}

impl LangChoice {
    /// `system_primary`: the primary language id of the Windows UI language (0x1F = Turkish).
    pub fn resolve(self, system_primary: u16) -> Lang {
        match self {
            LangChoice::En => Lang::En,
            LangChoice::Tr => Lang::Tr,
            LangChoice::System if system_primary == 0x1F => Lang::Tr,
            LangChoice::System => Lang::En,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            LangChoice::System => "system",
            LangChoice::En => "en",
            LangChoice::Tr => "tr",
        }
    }
}

pub const MONTHS_EN: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
pub const MONTHS_TR: [&str; 12] = ["Oca", "Şub", "Mar", "Nis", "May", "Haz", "Tem", "Ağu", "Eyl", "Eki", "Kas", "Ara"];

macro_rules! strings {
    ($($id:ident => $en:expr, $tr:expr;)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum S { $($id),* }
        pub const ALL: &[S] = &[$(S::$id),*];
        impl S {
            pub fn get(self, l: Lang) -> &'static str {
                match (self, l) {
                    $((S::$id, Lang::En) => $en, (S::$id, Lang::Tr) => $tr,)*
                }
            }
        }
    };
}

strings! {
    PlainHint => "Drag an area or click a window", "Bir alan sürükleyin ya da bir pencereye tıklayın";
    PlainKeyFull => "full screen", "tam ekran";
    RecordHint => "Pick the area to record", "Kaydedilecek alanı seçin";
    PlainKeyWork => "without the taskbar", "görev çubuğu olmadan";
    PlainKeyShiftClick => "Shift+click", "Shift+tık";
    PlainKeyAdd => "add one more", "bir tane daha ekle";
    PlainKeyRemove => "remove the last", "sonuncuyu sil";
    PlainKeyParts => "inner parts (or the wheel)", "iç parçalar (ya da tekerlek)";
    // Plain snip mode bar
    ModeRegion => "Region", "Alan";
    ModeWindow => "Window", "Pencere";
    ModeScreen => "Full screen", "Tam ekran";
    // Editor tools
    ToolArrow => "Arrow", "Ok";
    ToolRect => "Rectangle", "Dikdörtgen";
    ToolLine => "Line", "Çizgi";
    ToolPen => "Pen", "Kalem";
    ToolMarker => "Highlighter", "Vurgulayıcı";
    ToolText => "Text", "Metin";
    ToolSteps => "Numbered steps", "Numaralı adımlar";
    ToolObscure => "Pixelate", "Pikselleştir";
    ToolBlur => "Blur", "Bulanıklaştır";
    ToolSelect => "Select and move", "Seç ve taşı";
    ToolEllipse => "Ellipse", "Elips";
    ToolSpotlight => "Spotlight", "Spot ışığı";
    ToolPicker => "Pick a colour", "Renk al";
    FrameTip => "Frame the picture", "Resmi çerçevele";
    PickedColor => "Colour", "Renk";
    Color => "Colour", "Renk";
    CustomColor => "Custom colour…", "Özel renk…";
    Size => "Size", "Boyut";
    StrokeSize => "Stroke thickness", "Çizgi kalınlığı";
    FontSize => "Font size", "Yazı boyutu";
    Undo => "Undo", "Geri al";
    Redo => "Redo", "Yinele";
    Copy => "Copy", "Kopyala";
    Save => "Save", "Kaydet";
    SaveAs => "Save as…", "Farklı kaydet…";
    Pin => "Pin to screen", "Ekrana iğnele";
    Close => "Close", "Kapat";
    Copied => "Copied", "Kopyalandı";
    SnipProblem => "Snip: the capture was not fully saved", "Snip: alıntı tam kaydedilemedi";
    SnipProblemText => "The clipboard or a file could not be written. Details are in the log.", "Pano veya dosya yazılamadı. Ayrıntı günlükte.";
    Saved => "Saved", "Kaydedildi";
    // Tray
    SnipNow => "Snip", "Ekran alıntısı";
    OpenEditor => "Snip and edit", "Alıntı al ve düzenle";
    SnipIn3 => "Snip in 3 seconds", "3 saniye sonra alıntı";
    SnipIn5 => "Snip in 5 seconds", "5 saniye sonra alıntı";
    History => "Snip history", "Alıntı geçmişi";
    Settings => "Settings", "Ayarlar";
    OpenFolder => "Open screenshots folder", "Ekran görüntüleri klasörünü aç";
    OpenLog => "Open log", "Günlüğü aç";
    Rearm => "Set up the keys again", "Tuşları yeniden kur";
    Quit => "Quit Snip", "Snip'ten çık";
    StatusArmed => "Win+Shift+S ready", "Win+Shift+S hazır";
    StatusOff => "Win+Shift+S off", "Win+Shift+S kapalı";
    StatusFailed => "Win+Shift+S not armed", "Win+Shift+S kurulamadı";
    StatusArming => "Arming Win+Shift+S…", "Win+Shift+S kuruluyor…";
    StatusKeyMissing => "not registered; see Settings", "kurulamadı; Ayarlar'a bakın";
    Countdown => "Snip in", "Alıntıya kalan";
    // History window
    HistoryEmpty => "No captures yet", "Henüz alıntı yok";
    HistoryEmptyHint => "Press Win+Shift+S; your last 50 captures stay here.", "Win+Shift+S'ye basın; son 50 alıntı burada kalır.";
    OpenFolderShort => "Open folder", "Klasörü aç";
    Today => "Today", "Bugün";
    Yesterday => "Yesterday", "Dün";
    HistoryCount => "captures", "alıntı";
    Open => "Open", "Aç";
    ShowInFolder => "Show in folder", "Klasörde göster";
    Remove => "Remove from history", "Geçmişten kaldır";
    FileMissing => "File is gone", "Dosya yok";
    // Settings window
    EditorHotkey => "Snip and edit key", "Alıntı al ve düzenle tuşu";
    EditorHotkeyHint => "Choose an area, then add arrows, boxes and text", "Alan seçip üstüne ok, kutu, yazı eklersiniz";
    PressKeys => "Press a key combination…", "Bir tuş birleşimine basın…";
    HotkeyNone => "None", "Yok";
    HotkeyRefused => "Not allowed", "İzin verilmiyor";
    PrintScreenConflict => "Windows may use PrtScn for snipping", "Windows PrtScn’yi ekran alıntısı için kullanabilir";
    HotkeyInUse => "Already used by another program", "Başka bir program kullanıyor";
    HotkeyTaken => "Snip already uses this key for another job", "Snip bu tuşu başka bir iş için kullanıyor";
    WinShiftS => "Win+Shift+S takes a snip", "Win+Shift+S alıntı alır";
    WinShiftSHint => "Region, window or one screen, straight to the clipboard", "Alan, pencere veya tek ekran, doğrudan panoya";
    SaveFiles => "Save PNG files", "PNG dosyası kaydet";
    Folder => "Folder", "Klasör";
    FolderDefault => "Screenshots (Windows default)", "Ekran Görüntüleri (Windows varsayılanı)";
    Change => "Change…", "Değiştir…";
    CopyToClipboard => "Copy to clipboard", "Panoya kopyala";
    KeepHistory => "Keep snip history (last 50)", "Alıntı geçmişini tut (son 50)";
    Theme => "Theme", "Tema";
    ThemeSystem => "System", "Sistem";
    ThemeLight => "Light", "Açık";
    ThemeDark => "Dark", "Koyu";
    Language => "Language", "Dil";
    LangSystem => "System", "Sistem";
    Look => "Screenshot colour", "Ekran alıntısı rengi";
    LookHint => "Its button and selection frame. Copy text stays yellow.", "Düğmesi ve seçim çerçevesi. Metni kopyala hep sarı kalır.";
    SectionKeys => "Shortcuts", "Kısayollar";
    SectionSaving => "After a capture", "Alıntıdan sonra";
    SectionLooks => "Appearance and language", "Görünüm ve dil";
    LookGraphite => "Blue", "Mavi";
    LookPaper => "Black and white", "Siyah-beyaz";
    On => "On", "Açık";
    Off => "Off", "Kapalı";
    SettingsSaved => "Saved. Applies to the next snip.", "Kaydedildi. Bir sonraki alıntıda geçerli.";
    NoNetwork => "Snip never connects to the internet.", "Snip hiçbir zaman internete bağlanmaz.";
    // Main window
    AppLine => "Takes a picture or the text from your screen. Each job works on its own.", "Ekrandan görüntü ya da metin alır. Her iş ayrı çalışır.";
    GroupImage => "Picture of the screen", "Ekrandan görüntü";
    GroupText => "Text from the screen", "Ekrandan metin";
    FnSnip => "Screen snip", "Ekran alıntısı";
    FnSnipLine => "Choose an area, a window or one screen. The picture goes straight to the clipboard.", "Alan, pencere ya da tek ekran seçin. Görüntü hemen panoya gelir.";
    FnEdit => "Snip and edit", "Alıntı al ve düzenle";
    FnEditLine => "Choose an area, add arrows, boxes and text. Then copy or save it.", "Alan seçin, üstüne ok, kutu, yazı ekleyin. Sonra kopyalayın ya da kaydedin.";
    AllScreens => "All screens", "Tüm ekranlar";
    FnAllLine => "No selection. Every screen goes to the clipboard as one picture and is saved.", "Seçim yok. Bütün ekranlar tek resim olarak panoya gelir ve kaydedilir.";
    TakeNow => "Take now", "Şimdi al";
    ToastAll => "All screens on the clipboard", "Tüm ekranlar panoda";
    ToastAllSaved => "All screens saved", "Tüm ekranlar kaydedildi";
    ToastAllFailed => "All screens could not be taken", "Tüm ekranlar alınamadı";
    ToastAllFailedBody => "The screens could not be read. Details are in the log.", "Ekranlar okunamadı. Ayrıntı günlükte.";
    ScreensWord => "screens", "ekran";
    ScreenWord => "screen", "ekran";
    FnDelay => "Delayed snip", "Gecikmeli alıntı";
    FnDelayLine => "Gives you time to open a menu. When the time is up you choose the area.", "Bir menüyü açmanız için süre tanır. Süre bitince alanı seçersiniz.";
    FnText => "Copy text", "Metni kopyala";
    FnTextLine => "Drag over text you cannot select. It reaches the clipboard as text; no picture is kept.", "Seçemediğiniz yazının üstünden sürükleyin. Panoya metin olarak gelir; görüntü saklanmaz.";
    ChooseArea => "Choose area", "Alan seç";
    ChooseText => "Choose text", "Yazı seç";
    Seconds3 => "3 sec", "3 sn";
    Seconds5 => "5 sec", "5 sn";
    NoKeySet => "No key", "Tuş yok";
    TurnedOff => "Turned off", "Kapalı";
    TurnOn => "Turn on", "Aç";
    RecentSnips => "Recent snips", "Son alıntılar";
    RecentTexts => "Recent texts", "Son metinler";
    SeeAll => "See all", "Tümünü gör";
    NoSnipsYet => "Your snips appear here.", "Aldığınız görüntüler burada görünür.";
    NoTextsYet => "Texts you copy appear here. Click one to copy it again.", "Kopyaladığınız metinler burada görünür. Tıklayınca yeniden kopyalanır.";
    TextsOff => "Recent texts are not kept. You can change this in Settings.", "Son metinler saklanmıyor. Ayarlar'dan değiştirebilirsiniz.";
    TrayNotRunning => "Snip is not running in the background, so its keys do not work.", "Snip arka planda çalışmıyor, bu yüzden tuşlar çalışmaz.";
    StartSnip => "Start", "Başlat";
    OpenSnip => "Open Snip", "Snip'i aç";
    Troubleshooting => "Troubleshooting", "Sorun giderme";
    StatusReady => "Snip is ready", "Snip hazır";
    StatusKeysProblem => "Some keys did not start; see Settings", "Bazı tuşlar kurulamadı; Ayarlar'a bakın";
    // Copy text: the overlay
    TextHintDrag => "Drag over the text to copy", "Kopyalanacak metnin üzerinden sürükleyin";
    TextHintCancel => "cancel", "iptal";
    TextHintCross => "hold to cross screens", "basılıyken başka ekrana geçer";
    TooSmall => "too small", "çok küçük";
    ModeText => "Copy text", "Metni kopyala";
    CopyTextTip => "Copy text (Ctrl+Shift+C)", "Metni kopyala (Ctrl+Shift+C)";
    // Copy text: the result card
    CardCopied => "Copied", "Kopyalandı";
    CardMeta => "{0} lines, {1} characters", "{0} satır, {1} karakter";
    CardNoText => "Could not read any text", "Metin okunamadı";
    CardNoTextBody => "The clipboard did not change.", "Pano değişmedi.";
    CardNoTextTip => "Try a wider area, or choose the text language in Settings.", "Daha geniş bir alan deneyin ya da Ayarlar'dan metin dilini seçin.";
    CardUnchanged => "Not copied", "Kopyalanmadı";
    CardUnchangedBody => "The text was read, but the clipboard did not change.", "Metin okundu ama pano değişmedi.";
    CardUnchangedTip => "Another program may be holding the clipboard. Try again.", "Pano başka bir programda açık olabilir. Yeniden deneyin.";
    CardError => "Could not read the text", "Metin alınamadı";
    CardErrorBody => "Nothing was copied. Details are in the log.", "Hiçbir şey kopyalanmadı. Ayrıntı günlükte.";
    CardBusy => "Busy", "Meşgul";
    CardBusyBody => "Screen selection is already open.", "Ekran seçimi zaten açık.";
    CardNoEngine => "This language is not on this PC", "Bu dil bu bilgisayarda yüklü değil";
    CardNoEngineBody => "The clipboard did not change.", "Pano değişmedi.";
    CardNoEngineTip => "Choose another language in Settings, or add this one to Windows.", "Ayarlar'dan başka dil seçin ya da Windows'a bu dili ekleyin.";
    // Settings: tabs and new rows
    TabGeneral => "General", "Genel";
    SnipEnabled => "Screen snip", "Ekran alıntısı";
    SnipEnabledHint => "When off, its keys and menu entries go away; Win+Shift+S opens Windows' tool again.", "Kapalıyken tuşlar ve menü kalkar; Win+Shift+S yine Windows'un aracını açar.";
    Editor2Hotkey => "Second key for snip and edit", "Alıntı al ve düzenle için ikinci tuş";
    AllHotkey => "All screens, one picture", "Tüm ekranlar, tek resim";
    AllHotkeyHint => "No selection: every screen goes to the clipboard at once", "Seçim yok: bütün ekranlar tek seferde panoya gelir";
    AllHotkey2 => "Second key for all screens", "Tüm ekranlar için ikinci tuş";
    AllHotkey2Hint => "Does the same as the key above", "Üstteki tuşla aynı işi yapar";
    Editor2HotkeyHint => "Does the same as the editor key above", "Üstteki düzenleyici tuşuyla aynı işi yapar";
    TextEnabled => "Copy text", "Metni kopyala";
    TextEnabledHint => "When off, its key and menu entry go away. Screenshots keep working.", "Kapalıyken tuşu ve menüsü kalkar. Ekran alıntısı çalışmaya devam eder.";
    TextHotkey => "Copy text key", "Metin kopyalama tuşu";
    TextHotkeyHint => "Opens the screen for choosing text", "Yazı seçmek için ekranı açar";
    OcrLanguage => "Text language", "Metin dili";
    OcrLanguageHint => "The language of the text on the screen", "Ekrandaki yazının dili";
    OcrAuto => "Automatic (Windows language)", "Otomatik (Windows dili)";
    OcrUnavailable => "Not available", "Kullanılamıyor";
    KeepTexts => "Keep recent texts (last 20)", "Son metinleri sakla (son 20)";
    KeepTextsHint => "They stay on this PC. Turning this off deletes the list.", "Bu bilgisayarda kalır. Bu ayarı kapatınca liste silinir.";
    TextStaysHere => "Text is read on this PC and goes nowhere else.", "Metin bu bilgisayarda okunur, başka yere gitmez.";
    StartWithWindows => "Start with Windows", "Windows ile başlat";
    StartWithWindowsHint => "Keeps the keys ready after you sign in", "Oturum açınca tuşlar hazır olur";
    AutostartFailed => "Could not change the Windows start setting", "Windows başlangıç ayarı değiştirilemedi";
    SectionKeysAndUse => "Keys", "Tuşlar";
    SectionReading => "Reading", "Okuma";
    SectionProgram => "Program", "Program";
    Version => "Version", "Sürüm";
    // Pin window
    Opacity => "Opacity", "Saydamlık";
    ZoomReset => "Actual size", "Gerçek boyut";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_string_has_both_languages() {
        assert!(ALL.len() > 40);
        for s in ALL {
            for l in [Lang::En, Lang::Tr] {
                assert!(!s.get(l).trim().is_empty(), "{s:?} {l:?} is empty");
            }
        }
    }

    #[test]
    fn turkish_uses_turkish_letters() {
        let tr: String = ALL.iter().map(|s| s.get(Lang::Tr)).collect();
        for ch in ['ı', 'ş', 'ğ', 'ü', 'ö', 'ç'] {
            assert!(tr.contains(ch), "no {ch} anywhere: ASCII stand-ins?");
        }
        // Common ASCII mistakes for words that appear above.
        for wrong in [
            "Gecmis",
            "Klasor",
            "Saydamlik",
            "Dikdortgen",
            "Cizgi",
            "Vurgulayici",
        ] {
            assert!(!tr.contains(wrong), "ASCII stand-in {wrong}");
        }
    }

    #[test]
    fn system_language_picks_turkish_only_for_turkish() {
        assert_eq!(LangChoice::System.resolve(0x1F), Lang::Tr);
        assert_eq!(LangChoice::System.resolve(0x09), Lang::En);
        assert_eq!(LangChoice::System.resolve(0x07), Lang::En);
        assert_eq!(LangChoice::En.resolve(0x1F), Lang::En);
        assert_eq!(LangChoice::Tr.resolve(0x09), Lang::Tr);
    }

    #[test]
    fn english_and_turkish_differ_for_most_strings() {
        let same = ALL
            .iter()
            .filter(|s| s.get(Lang::En) == s.get(Lang::Tr))
            .count();
        assert!(
            same * 10 < ALL.len(),
            "{same} of {} strings are not translated",
            ALL.len()
        );
    }
}
