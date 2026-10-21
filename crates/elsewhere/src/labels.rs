//! What the columns of a playlist CSV may be called. Exportify writes every label in the language
//! of the person who ran it (its `src/i18n/locales/*/translation.json`), so each is listed in all
//! of them, lower case.

/// The song title column.
pub(crate) const TITLE: &[&str] = &[
    "track name",
    "track",
    "title",
    "song",
    "song title",
    "track title",
    "name",
    "låtens namn",
    "nom du titre",
    "nombre de la canción",
    "nome da faixa",
    "nome della traccia",
    "nummernaam",
    "parça adı",
    "track-name",
    "όνομα κομματιού",
    "اسم الأغنية",
    "トラック名",
    "曲目名称",
    "曲目名稱",
    "歌名",
];

/// The artists column. Google's numbered "artist name 1" columns are matched apart from these.
pub(crate) const ARTIST: &[&str] = &[
    "artist name(s)",
    "artist names",
    "artist name",
    "artists",
    "artist",
    "artist 名",
    "artistens namn",
    "künstlername(n)",
    "naam van artiest",
    "nom(s) de l'artiste",
    "nombre(s) del artista",
    "nome dell'artista",
    "nome(s) do artista",
    "sanatçı adı",
    "όνομα/τα καλλιτέχνη",
    "أسماء الفنانين",
    "アーティスト名",
    "艺人名称",
    "藝人名稱",
];

/// The album column.
pub(crate) const ALBUM: &[&str] = &[
    "album name",
    "album",
    "album title",
    "album-name",
    "albumets namn",
    "albüm adı",
    "naam van album",
    "nom de l'album",
    "nombre del álbum",
    "nome dell'album",
    "nome do álbum",
    "όνομα άλμπουμ",
    "اسم الألبوم",
    "アルバム名",
    "专辑名称",
    "專輯名",
    "專輯名稱",
];

/// The length column, in milliseconds.
pub(crate) const DURATION: &[&str] = &[
    "track duration (ms)",
    "duration (ms)",
    "duration_ms",
    "duration ms",
    "duración de la canción (ms)",
    "durata della traccia (ms)",
    "duração da faixa (ms)",
    "durée du titre (ms)",
    "låtlängd (ms)",
    "nummerduur (ms)",
    "parça süresi (ms)",
    "track-dauer (ms)",
    "διάρκεια κομματιού (ms)",
    "مدة الأغنية (بالمللي ثانية)",
    "トラックの長さ（ミリ秒）",
    "曲目时长（毫秒）",
    "曲目時長（毫秒）",
    "曲目長度（毫秒）",
];

/// The service's id for the song.
pub(crate) const ID: &[&str] = &[
    "track uri",
    "spotify id",
    "track id",
    "video id",
    "uri",
    "låtens uri",
    "nummer uri",
    "parça uri",
    "track-uri",
    "uri da faixa",
    "uri de la canción",
    "uri della traccia",
    "uri du titre",
    "uri κομματιού",
    "رابط الأغنية (uri)",
    "トラックuri",
    "曲目 uri",
    "歌 link",
];
