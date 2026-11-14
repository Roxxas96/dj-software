//! The cover services: Deezer's public search first, then a MusicBrainz recording search
//! with the Cover Art Archive's front image. Each reaches the network only through
//! [`Http`], so their URLs and their reading of the answers are tested from recorded
//! answers, without the network.
//!
//! A request carries the query text (artist and title, the title, or the file name without
//! its extension) in its URL and the [`USER_AGENT`], nothing else: no path, no account, no
//! cookie. Every answer is untrusted: its size is capped, only `https` image URLs from
//! known hosts are followed, and the image itself is decoded with limits by the caller.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use library::CoverQuery;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;

/// What every request says it comes from, as MusicBrainz asks; Deezer gets the same.
pub const USER_AGENT: &str = concat!(
    "OpenJockey/",
    env!("CARGO_PKG_VERSION"),
    " ( https://github.com/Roxxas96/open-jockey )"
);
/// The most a service's JSON answer may weigh.
pub const JSON_LIMIT: u64 = 2 * 1024 * 1024;
/// The most an image may weigh.
pub const IMAGE_LIMIT: u64 = 10 * 1024 * 1024;
/// The longest a request may take, from connecting to the last byte.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest connecting may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The least time between two requests to MusicBrainz or the Cover Art Archive: they ask
/// for at most one a second.
pub const MUSICBRAINZ_GAP: Duration = Duration::from_millis(1100);
/// The least time between two requests to Deezer: at most 5 a second.
pub const DEEZER_GAP: Duration = Duration::from_millis(200);
/// How often a wait between two requests checks whether the lookups were turned off.
const PACE_SLICE: Duration = Duration::from_millis(50);
/// How many pictures one service fetches for one lookup, at most.
const CANDIDATES: usize = 3;
/// The lowest MusicBrainz search score a recording is taken at.
const MIN_SCORE: u32 = 80;

/// Why an HTTP request gave nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    /// The server answered with this status.
    Status(u16),
    /// No answer: no network, a name that does not resolve, a timeout, a dropped
    /// connection.
    Network(String),
    /// The answer is larger than allowed.
    TooLarge,
    /// The URL (or a redirect) is not `https`.
    Refused(String),
}

/// A GET over HTTPS, returning the body of a 2xx answer of at most `limit` bytes. The only
/// thing a request carries besides `url` is the [`USER_AGENT`].
pub trait Http: Send + Sync {
    /// Fetches `url`.
    ///
    /// # Errors
    ///
    /// See [`HttpError`].
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>, HttpError>;
}

/// [`Http`] over the network: the OS trust store, `https` only (redirects too), a
/// [`REQUEST_TIMEOUT`] for the whole request, no cookies.
pub struct UreqHttp {
    agent: ureq::Agent,
}

impl UreqHttp {
    pub fn new() -> Self {
        use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
        let agent = ureq::Agent::config_builder()
            .user_agent(USER_AGENT)
            .https_only(true)
            .max_redirects(3)
            .timeout_global(Some(REQUEST_TIMEOUT))
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .tls_config(
                TlsConfig::builder()
                    .provider(TlsProvider::NativeTls)
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .build()
            .new_agent();
        Self { agent }
    }
}

impl Http for UreqHttp {
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>, HttpError> {
        if !url.starts_with("https://") {
            return Err(HttpError::Refused(url.to_owned()));
        }
        let failed = |e: ureq::Error| match e {
            ureq::Error::StatusCode(code) => HttpError::Status(code),
            ureq::Error::BodyExceedsLimit(_) => HttpError::TooLarge,
            ureq::Error::RequireHttpsOnly(url) => HttpError::Refused(url),
            other => HttpError::Network(other.to_string()),
        };
        let mut response = self.agent.get(url).call().map_err(failed)?;
        if response
            .body()
            .content_length()
            .is_some_and(|length| length > limit)
        {
            return Err(HttpError::TooLarge);
        }
        response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(failed)
    }
}

/// Why a lookup gave no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupError {
    /// The service could not be reached, timed out, failed (5xx) or asked to slow down:
    /// nothing is recorded, the track is looked up again later and lookups pause for a
    /// while.
    Unreachable(String),
    /// The service answered something unusable that asking again will not change (a 4xx,
    /// an error of its own, an answer that does not parse): for this track it counts as
    /// "no cover".
    Failed(String),
    /// The lookups were turned off, or the app is quitting: no further request was made.
    Cancelled,
}

impl LookupError {
    /// Whether lookups should pause before the next request.
    pub fn is_unreachable(&self) -> bool {
        matches!(self, Self::Unreachable(_))
    }
}

impl std::fmt::Display for LookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(why) | Self::Failed(why) => f.write_str(why),
            Self::Cancelled => f.write_str("cancelled"),
        }
    }
}

/// A place covers are looked up in.
pub trait CoverService: Send {
    /// The image bytes of a cover for `query`, `None` when the service has none.
    ///
    /// # Errors
    ///
    /// See [`LookupError`].
    #[allow(
        dead_code,
        reason = "the plain lookup; the worker always says which pictures it takes"
    )]
    fn find(&self, query: &CoverQuery) -> Result<Option<Vec<u8>>, LookupError> {
        self.find_other(query, &|_| true, &|| true)
    }

    /// [`CoverService::find`], skipping the pictures `accept` turns down (one that does not
    /// decode, or the cover the track already has): the first picture it accepts, `None`
    /// when there is no other. `go_on` is asked before every request and every wait
    /// between two: once it says no, no further request is made
    /// ([`LookupError::Cancelled`]).
    ///
    /// # Errors
    ///
    /// See [`LookupError`].
    fn find_other(
        &self,
        query: &CoverQuery,
        accept: &dyn Fn(&[u8]) -> bool,
        go_on: &dyn Fn() -> bool,
    ) -> Result<Option<Vec<u8>>, LookupError>;
}

/// Several services, asked in order. A cover from one is the answer. A service that could
/// not be reached makes the answer that error (the track is looked up again later);
/// otherwise it is "none", a service's [`LookupError::Failed`] counting as its "none".
pub struct Chain(pub Vec<Box<dyn CoverService>>);

impl CoverService for Chain {
    fn find_other(
        &self,
        query: &CoverQuery,
        accept: &dyn Fn(&[u8]) -> bool,
        go_on: &dyn Fn() -> bool,
    ) -> Result<Option<Vec<u8>>, LookupError> {
        let mut unreachable = None;
        for service in &self.0 {
            match service.find_other(query, accept, go_on) {
                Ok(Some(bytes)) => return Ok(Some(bytes)),
                Ok(None) | Err(LookupError::Failed(_)) => {}
                Err(LookupError::Cancelled) => return Err(LookupError::Cancelled),
                Err(e @ LookupError::Unreachable(_)) => {
                    unreachable.get_or_insert(e);
                }
            }
        }
        unreachable.map_or(Ok(None), Err)
    }
}

/// Deezer and MusicBrainz over the network, in that order.
pub fn online() -> Chain {
    let http: Arc<dyn Http> = Arc::new(UreqHttp::new());
    Chain(vec![
        Box::new(DeezerService::new(Arc::clone(&http), DEEZER_GAP)),
        Box::new(MusicBrainzService::new(http, MUSICBRAINZ_GAP)),
    ])
}

/// Keeps requests at least `gap` apart.
struct Pace {
    gap: Duration,
    next: Mutex<Option<Instant>>,
}

impl Pace {
    fn new(gap: Duration) -> Self {
        Self {
            gap,
            next: Mutex::new(None),
        }
    }

    /// Waits until the next request may go, and books the one after. `go_on` is asked
    /// before, during (every [`PACE_SLICE`]) and after the wait: once it says no, nothing
    /// is booked and the request must not be made.
    fn wait(&self, go_on: &dyn Fn() -> bool) -> Result<(), LookupError> {
        let mut next = self.next.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if !go_on() {
                return Err(LookupError::Cancelled);
            }
            let now = Instant::now();
            match *next {
                Some(at) if at > now => std::thread::sleep((at - now).min(PACE_SLICE)),
                _ => break,
            }
        }
        *next = Some(Instant::now() + self.gap);
        Ok(())
    }
}

/// The text of a lookup: `artist title` when the track has a title tag (the title alone
/// without an artist), else the file name without its extension. Blank tags count as none.
pub fn query_text(query: &CoverQuery) -> String {
    match tags(query) {
        (Some(artist), Some(title)) => format!("{artist} {title}"),
        (_, Some(title)) => title.to_owned(),
        _ => query.file_stem.trim().to_owned(),
    }
}

/// The track's artist and title, without blank ones.
fn tags(query: &CoverQuery) -> (Option<&str>, Option<&str>) {
    fn tag(t: Option<&str>) -> Option<&str> {
        t.map(str::trim).filter(|t| !t.is_empty())
    }
    (tag(query.artist.as_deref()), tag(query.title.as_deref()))
}

/// What a query value keeps unencoded: RFC 3986's unreserved characters.
const QUERY_VALUE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

fn encode(text: &str) -> String {
    utf8_percent_encode(text, QUERY_VALUE).to_string()
}

/// Maps a failed request for a service's JSON answer.
fn json_error(service: &str, e: HttpError) -> LookupError {
    match e {
        HttpError::Network(why) => LookupError::Unreachable(format!("{service}: {why}")),
        HttpError::Status(code) if code >= 500 || code == 429 => {
            LookupError::Unreachable(format!("{service}: HTTP {code}"))
        }
        HttpError::Status(code) => LookupError::Failed(format!("{service}: HTTP {code}")),
        HttpError::TooLarge => LookupError::Failed(format!("{service}: the answer is too large")),
        HttpError::Refused(url) => LookupError::Failed(format!("{service}: refused {url}")),
    }
}

/// Fetches the images at `urls` in turn: the first one `accept` takes. A picture that is
/// gone, too large or refused is skipped; no answer at all is an error.
fn first_image(
    service: &str,
    http: &dyn Http,
    pace: &Pace,
    urls: &[String],
    accept: &dyn Fn(&[u8]) -> bool,
    go_on: &dyn Fn() -> bool,
) -> Result<Option<Vec<u8>>, LookupError> {
    for url in urls.iter().take(CANDIDATES) {
        pace.wait(go_on)?;
        match http.get(url, IMAGE_LIMIT) {
            Ok(bytes) if accept(&bytes) => return Ok(Some(bytes)),
            Ok(_) | Err(HttpError::TooLarge | HttpError::Refused(_)) => {}
            Err(HttpError::Status(code)) if code < 500 && code != 429 => {}
            Err(e) => return Err(json_error(service, e)),
        }
    }
    Ok(None)
}

/// Deezer's public search (no key): `https://api.deezer.com/search?q=artist:"…" track:"…"`.
pub struct DeezerService {
    http: Arc<dyn Http>,
    pace: Pace,
}

impl DeezerService {
    /// Deezer through `http`, its requests at least `gap` apart.
    pub fn new(http: Arc<dyn Http>, gap: Duration) -> Self {
        Self {
            http,
            pace: Pace::new(gap),
        }
    }

    /// The search URL for `query`; `None` when there is nothing to search for.
    pub fn search_url(query: &CoverQuery) -> Option<String> {
        // Deezer's search has no escape inside a quoted name: a quote in one is dropped.
        let unquoted = |text: &str| text.replace('"', " ");
        let q = match tags(query) {
            (Some(artist), Some(title)) => format!(
                "artist:\"{}\" track:\"{}\"",
                unquoted(artist),
                unquoted(title)
            ),
            _ => unquoted(&query_text(query)).trim().to_owned(),
        };
        (!q.is_empty()).then(|| format!("https://api.deezer.com/search?q={}&limit=10", encode(&q)))
    }

    /// The cover URLs in a search answer, best first, each once: `https` URLs on Deezer's
    /// image servers only.
    ///
    /// # Errors
    ///
    /// An answer that is not a search result, or Deezer's error (its quota and busy errors
    /// are [`LookupError::Unreachable`]; its "no data" is no error, just no cover).
    pub fn covers(answer: &[u8]) -> Result<Vec<String>, LookupError> {
        #[derive(Deserialize)]
        struct Answer {
            data: Option<Vec<Track>>,
            error: Option<ApiError>,
        }
        #[derive(Deserialize)]
        struct ApiError {
            code: Option<i64>,
            message: Option<String>,
        }
        #[derive(Deserialize)]
        struct Track {
            album: Option<Album>,
        }
        #[derive(Deserialize)]
        struct Album {
            cover_big: Option<String>,
            cover_xl: Option<String>,
            cover_medium: Option<String>,
        }
        let answer: Answer = serde_json::from_slice(answer)
            .map_err(|e| LookupError::Failed(format!("Deezer: {e}")))?;
        if let Some(error) = answer.error {
            let why = format!("Deezer: {}", error.message.as_deref().unwrap_or("an error"));
            return match error.code {
                // "Quota limit exceeded" and "service busy": too many requests for now.
                Some(4 | 700) => Err(LookupError::Unreachable(why)),
                // "Data not found": no cover.
                Some(800) => Ok(Vec::new()),
                _ => Err(LookupError::Failed(why)),
            };
        }
        let Some(tracks) = answer.data else {
            return Err(LookupError::Failed(
                "Deezer: no results in the answer".into(),
            ));
        };
        let mut urls: Vec<String> = Vec::new();
        for album in tracks.into_iter().filter_map(|t| t.album) {
            let url = [album.cover_big, album.cover_xl, album.cover_medium]
                .into_iter()
                .flatten()
                .find(|url| deezer_image(url));
            if let Some(url) = url
                && !urls.contains(&url)
            {
                urls.push(url);
            }
        }
        Ok(urls)
    }
}

/// Whether `url` is an `https` URL on Deezer's image servers.
fn deezer_image(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !host.contains(['@', ':']) && (host.ends_with(".dzcdn.net") || host == "dzcdn.net")
}

impl CoverService for DeezerService {
    fn find_other(
        &self,
        query: &CoverQuery,
        accept: &dyn Fn(&[u8]) -> bool,
        go_on: &dyn Fn() -> bool,
    ) -> Result<Option<Vec<u8>>, LookupError> {
        let Some(url) = Self::search_url(query) else {
            return Ok(None);
        };
        self.pace.wait(go_on)?;
        let answer = self
            .http
            .get(&url, JSON_LIMIT)
            .map_err(|e| json_error("Deezer", e))?;
        let covers = Self::covers(&answer)?;
        first_image(
            "Deezer",
            self.http.as_ref(),
            &self.pace,
            &covers,
            accept,
            go_on,
        )
    }
}

/// A MusicBrainz recording search, then the Cover Art Archive's front image of the
/// recordings' releases.
pub struct MusicBrainzService {
    http: Arc<dyn Http>,
    pace: Pace,
}

impl MusicBrainzService {
    /// MusicBrainz and the Cover Art Archive through `http`, their requests at least `gap`
    /// apart (MusicBrainz asks for [`MUSICBRAINZ_GAP`]).
    pub fn new(http: Arc<dyn Http>, gap: Duration) -> Self {
        Self {
            http,
            pace: Pace::new(gap),
        }
    }

    /// The recording search URL for `query`; `None` when there is nothing to search for.
    pub fn search_url(query: &CoverQuery) -> Option<String> {
        let lucene = match tags(query) {
            (Some(artist), Some(title)) => {
                format!("artist:{} AND recording:{}", phrase(artist), phrase(title))
            }
            (_, Some(title)) => format!("recording:{}", phrase(title)),
            _ => free_text(query.file_stem.trim()),
        };
        (!lucene.trim().is_empty()).then(|| {
            format!(
                "https://musicbrainz.org/ws/2/recording?query={}&fmt=json&limit=5",
                encode(&lucene)
            )
        })
    }

    /// The Cover Art Archive URLs of the releases of the well-matched recordings in a
    /// search answer, best first, each once.
    ///
    /// # Errors
    ///
    /// An answer that is not a recording search result.
    pub fn covers(answer: &[u8]) -> Result<Vec<String>, LookupError> {
        #[derive(Deserialize)]
        struct Answer {
            recordings: Vec<Recording>,
        }
        #[derive(Deserialize)]
        struct Recording {
            score: Option<u32>,
            #[serde(default)]
            releases: Vec<Release>,
        }
        #[derive(Deserialize)]
        struct Release {
            id: String,
        }
        let answer: Answer = serde_json::from_slice(answer)
            .map_err(|e| LookupError::Failed(format!("MusicBrainz: {e}")))?;
        let mut urls: Vec<String> = Vec::new();
        let releases = answer
            .recordings
            .into_iter()
            .filter(|r| r.score.unwrap_or(0) >= MIN_SCORE)
            .flat_map(|r| r.releases);
        for release in releases.filter(|r| is_mbid(&r.id)) {
            let url = format!(
                "https://coverartarchive.org/release/{}/front-500",
                release.id
            );
            if !urls.contains(&url) {
                urls.push(url);
            }
        }
        Ok(urls)
    }
}

impl CoverService for MusicBrainzService {
    fn find_other(
        &self,
        query: &CoverQuery,
        accept: &dyn Fn(&[u8]) -> bool,
        go_on: &dyn Fn() -> bool,
    ) -> Result<Option<Vec<u8>>, LookupError> {
        let Some(url) = Self::search_url(query) else {
            return Ok(None);
        };
        self.pace.wait(go_on)?;
        let answer = self
            .http
            .get(&url, JSON_LIMIT)
            .map_err(|e| json_error("MusicBrainz", e))?;
        let covers = Self::covers(&answer)?;
        first_image(
            "Cover Art Archive",
            self.http.as_ref(),
            &self.pace,
            &covers,
            accept,
            go_on,
        )
    }
}

/// A MusicBrainz id: 36 characters, hex digits and dashes, in the 8-4-4-4-12 layout.
fn is_mbid(id: &str) -> bool {
    let parts: Vec<&str> = id.split('-').collect();
    parts.iter().map(|p| p.len()).eq([8, 4, 4, 4, 12])
        && parts
            .iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// `text` as a Lucene phrase: in quotes, its backslashes and quotes escaped.
fn phrase(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `text` as Lucene free text: every character with a meaning in a query escaped.
fn free_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "+-&|!(){}[]^\"~*?:\\/".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Recorded answers by URL prefix, and every URL asked for.
    #[derive(Default)]
    pub(crate) struct RecordingHttp {
        answers: Vec<(String, Result<Vec<u8>, HttpError>)>,
        pub asked: Mutex<Vec<String>>,
    }

    impl RecordingHttp {
        pub(crate) fn answer(mut self, prefix: &str, answer: Result<Vec<u8>, HttpError>) -> Self {
            self.answers.push((prefix.to_owned(), answer));
            self
        }

        pub(crate) fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl Http for RecordingHttp {
        fn get(&self, url: &str, _limit: u64) -> Result<Vec<u8>, HttpError> {
            self.asked.lock().unwrap().push(url.to_owned());
            self.answers
                .iter()
                .find(|(prefix, _)| url.starts_with(prefix.as_str()))
                .map_or(Err(HttpError::Status(404)), |(_, answer)| answer.clone())
        }
    }

    pub(crate) fn fixture(name: &str) -> Vec<u8> {
        let path = format!(
            "{}/tests/fixtures/covers/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read(path).unwrap()
    }

    fn query(artist: Option<&str>, title: Option<&str>, stem: &str) -> CoverQuery {
        CoverQuery {
            track: 4242,
            artist: artist.map(str::to_owned),
            title: title.map(str::to_owned),
            file_stem: stem.to_owned(),
        }
    }

    fn deezer(http: &Arc<RecordingHttp>) -> DeezerService {
        DeezerService::new(Arc::clone(http) as Arc<dyn Http>, Duration::ZERO)
    }

    fn musicbrainz(http: &Arc<RecordingHttp>) -> MusicBrainzService {
        MusicBrainzService::new(Arc::clone(http) as Arc<dyn Http>, Duration::ZERO)
    }

    const DISCOVERY: &str = "https://e-cdns-images.dzcdn.net/images/cover/2e018122cb56986277102d2041a592c8/500x500-000000-80-0-0.jpg";
    const ALIVE: &str = "https://e-cdns-images.dzcdn.net/images/cover/5718f7c81c27e0b2417e2a4c45224f8a/500x500-000000-80-0-0.jpg";

    #[test]
    fn deezer_and_musicbrainz_parse_their_answers() {
        // Deezer: the album covers, best size first; plain http and other hosts dropped.
        let covers = DeezerService::covers(&fixture("deezer_search.json")).unwrap();
        assert_eq!(covers, [DISCOVERY, ALIVE]);
        assert_eq!(
            DeezerService::covers(&fixture("deezer_empty.json")).unwrap(),
            Vec::<String>::new()
        );
        assert!(
            DeezerService::covers(&fixture("deezer_quota.json"))
                .unwrap_err()
                .is_unreachable()
        );
        assert!(matches!(
            DeezerService::covers(b"<html>"),
            Err(LookupError::Failed(_))
        ));
        // MusicBrainz: the well-scored recordings' releases, valid ids only.
        let covers = MusicBrainzService::covers(&fixture("musicbrainz_recording.json")).unwrap();
        assert_eq!(
            covers,
            [
                "https://coverartarchive.org/release/48140466-cff6-3222-bd55-63c27e43190d/front-500",
                "https://coverartarchive.org/release/7a1f4c2d-0e2b-4f8a-9c3d-1b2e3f4a5b6c/front-500",
            ]
        );
        assert!(
            MusicBrainzService::covers(&fixture("musicbrainz_empty.json"))
                .unwrap()
                .is_empty()
        );
        // Through a service: the search, then the first picture accepted.
        let http = Arc::new(
            RecordingHttp::default()
                .answer("https://api.deezer.com/", Ok(fixture("deezer_search.json")))
                .answer(DISCOVERY, Ok(b"discovery".to_vec()))
                .answer(ALIVE, Ok(b"alive".to_vec())),
        );
        let q = query(
            Some("Daft Punk"),
            Some("Harder, Better, Faster, Stronger"),
            "x",
        );
        assert_eq!(deezer(&http).find(&q).unwrap(), Some(b"discovery".to_vec()));
        // Another than the one it has.
        assert_eq!(
            deezer(&http)
                .find_other(&q, &|bytes| bytes != b"discovery", &|| true)
                .unwrap(),
            Some(b"alive".to_vec())
        );
        let http = Arc::new(
            RecordingHttp::default()
                .answer(
                    "https://musicbrainz.org/",
                    Ok(fixture("musicbrainz_recording.json")),
                )
                .answer(
                    "https://coverartarchive.org/release/7a1f4c2d",
                    Ok(b"single".to_vec()),
                ),
        );
        // The first release has no art (404): the next one's.
        assert_eq!(
            musicbrainz(&http).find(&q).unwrap(),
            Some(b"single".to_vec())
        );
        assert_eq!(http.asked().len(), 3);
    }

    #[test]
    fn only_artist_and_title_are_sent() {
        let http = Arc::new(
            RecordingHttp::default()
                .answer("https://api.deezer.com/", Ok(fixture("deezer_empty.json")))
                .answer(
                    "https://musicbrainz.org/",
                    Ok(fixture("musicbrainz_empty.json")),
                ),
        );
        let chain = Chain(vec![Box::new(deezer(&http)), Box::new(musicbrainz(&http))]);
        // The stem is that of a file in a folder with a private name.
        let tagged = query(Some("Daft Punk"), Some("One More Time"), "secret-stem");
        assert_eq!(chain.find(&tagged).unwrap(), None);
        let asked = http.asked();
        assert_eq!(
            asked,
            [
                "https://api.deezer.com/search?q=artist%3A%22Daft%20Punk%22%20track%3A%22One%20More%20Time%22&limit=10",
                "https://musicbrainz.org/ws/2/recording?query=artist%3A%22Daft%20Punk%22%20AND%20recording%3A%22One%20More%20Time%22&fmt=json&limit=5",
            ]
        );
        for url in &asked {
            assert!(!url.contains("secret"), "{url}");
            assert!(!url.contains("4242"), "{url}");
        }
    }

    #[test]
    fn a_track_without_tags_is_looked_up_by_its_file_name() {
        let http = Arc::new(
            RecordingHttp::default()
                .answer("https://api.deezer.com/", Ok(fixture("deezer_empty.json")))
                .answer(
                    "https://musicbrainz.org/",
                    Ok(fixture("musicbrainz_empty.json")),
                ),
        );
        let chain = Chain(vec![Box::new(deezer(&http)), Box::new(musicbrainz(&http))]);
        let q = query(None, Some("  "), "Artist - Song (Club Mix)");
        assert_eq!(query_text(&q), "Artist - Song (Club Mix)");
        assert_eq!(chain.find(&q).unwrap(), None);
        assert_eq!(
            http.asked(),
            [
                "https://api.deezer.com/search?q=Artist%20-%20Song%20%28Club%20Mix%29&limit=10",
                "https://musicbrainz.org/ws/2/recording?query=Artist%20%5C-%20Song%20%5C%28Club%20Mix%5C%29&fmt=json&limit=5",
            ]
        );
        // The title alone, without an artist.
        let q = query(None, Some("Solo"), "file");
        assert_eq!(query_text(&q), "Solo");
        assert_eq!(
            MusicBrainzService::search_url(&q).unwrap(),
            "https://musicbrainz.org/ws/2/recording?query=recording%3A%22Solo%22&fmt=json&limit=5"
        );
        // Nothing to send: no request.
        let http = Arc::new(RecordingHttp::default());
        assert_eq!(deezer(&http).find(&query(None, None, " ")).unwrap(), None);
        assert!(http.asked().is_empty());
    }

    #[test]
    fn quotes_in_names_cannot_change_the_query() {
        let q = query(Some("A\" OR artist:\"B"), Some("T\\\""), "x");
        let mb = MusicBrainzService::search_url(&q).unwrap();
        let query = mb
            .split("query=")
            .nth(1)
            .and_then(|q| q.split('&').next())
            .unwrap();
        let decoded = percent_encoding::percent_decode_str(query)
            .decode_utf8()
            .unwrap();
        assert_eq!(
            decoded,
            r#"artist:"A\" OR artist:\"B" AND recording:"T\\\"""#
        );
        let deezer = DeezerService::search_url(&q).unwrap();
        let decoded = percent_encoding::percent_decode_str(&deezer)
            .decode_utf8()
            .unwrap()
            .into_owned();
        assert!(
            decoded.contains(r#"artist:"A  OR artist: B" track:"T\ ""#),
            "{decoded}"
        );
        // Nothing breaks out of the query parameter.
        for (url, parameters) in [(mb, 2), (deezer, 1)] {
            assert_eq!(url.matches('&').count(), parameters, "{url}");
            assert!(!url.contains([' ', '"', '#', '\\']), "{url}");
        }
    }

    #[test]
    fn failures_are_not_misses() {
        let q = query(Some("A"), Some("T"), "x");
        let cases: [(Result<Vec<u8>, HttpError>, bool); 4] = [
            (Err(HttpError::Network("offline".into())), true),
            (Err(HttpError::Status(503)), true),
            (Err(HttpError::Status(429)), true),
            (Err(HttpError::Status(400)), false),
        ];
        for (answer, unreachable) in cases {
            let http = Arc::new(RecordingHttp::default().answer("https://", answer));
            let error = deezer(&http).find(&q).unwrap_err();
            assert_eq!(error.is_unreachable(), unreachable, "{error}");
        }
        // Deezer's busy error pauses the lookups; its "no data" is no cover; another of its
        // errors fails for good.
        let deezer_error = |code: i64| {
            let answer = format!(r#"{{"error":{{"type":"X","message":"m","code":{code}}}}}"#);
            DeezerService::covers(answer.as_bytes())
        };
        assert!(deezer_error(700).unwrap_err().is_unreachable());
        assert_eq!(deezer_error(800), Ok(Vec::new()));
        assert!(matches!(deezer_error(600), Err(LookupError::Failed(_))));
        // A service failing for good and the other finding nothing is no cover: asking
        // again would change nothing.
        let http = Arc::new(
            RecordingHttp::default()
                .answer("https://api.deezer.com/", Err(HttpError::Status(400)))
                .answer(
                    "https://musicbrainz.org/",
                    Ok(fixture("musicbrainz_empty.json")),
                ),
        );
        let chain = Chain(vec![Box::new(deezer(&http)), Box::new(musicbrainz(&http))]);
        assert_eq!(chain.find(&q), Ok(None));
        // One service unreachable and the other finding nothing is a failure, not a miss.
        let http = Arc::new(
            RecordingHttp::default()
                .answer(
                    "https://api.deezer.com/",
                    Err(HttpError::Network("x".into())),
                )
                .answer(
                    "https://musicbrainz.org/",
                    Ok(fixture("musicbrainz_empty.json")),
                ),
        );
        let chain = Chain(vec![Box::new(deezer(&http)), Box::new(musicbrainz(&http))]);
        assert!(chain.find(&q).unwrap_err().is_unreachable());
        // An image that went offline midway is a failure too; one that is gone is skipped.
        let http = Arc::new(
            RecordingHttp::default()
                .answer("https://api.deezer.com/", Ok(fixture("deezer_search.json")))
                .answer(DISCOVERY, Err(HttpError::Network("x".into()))),
        );
        assert!(deezer(&http).find(&q).unwrap_err().is_unreachable());
        let http = Arc::new(
            RecordingHttp::default()
                .answer("https://api.deezer.com/", Ok(fixture("deezer_search.json")))
                .answer(DISCOVERY, Err(HttpError::Status(404))),
        );
        assert_eq!(deezer(&http).find(&q).unwrap(), None);
    }

    #[test]
    fn turning_lookups_off_stops_the_next_request() {
        let http = Arc::new(
            RecordingHttp::default()
                .answer("https://api.deezer.com/", Ok(fixture("deezer_search.json")))
                .answer(
                    "https://musicbrainz.org/",
                    Ok(fixture("musicbrainz_recording.json")),
                ),
        );
        let chain = Chain(vec![Box::new(deezer(&http)), Box::new(musicbrainz(&http))]);
        let q = query(Some("A"), Some("T"), "x");
        // Turned off right after the first request: not one more, from any service.
        let asked = Arc::clone(&http);
        let error = chain
            .find_other(&q, &|_| true, &|| asked.asked().is_empty())
            .unwrap_err();
        assert_eq!(error, LookupError::Cancelled);
        assert_eq!(http.asked().len(), 1);
        // Turned off during the wait between two MusicBrainz requests: it ends at once.
        let http = Arc::new(RecordingHttp::default().answer(
            "https://musicbrainz.org/",
            Ok(fixture("musicbrainz_recording.json")),
        ));
        let service = MusicBrainzService::new(Arc::clone(&http) as Arc<dyn Http>, MUSICBRAINZ_GAP);
        let on = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let switch = Arc::clone(&on);
        let off = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            switch.store(false, std::sync::atomic::Ordering::Relaxed);
        });
        let began = Instant::now();
        let go_on = || on.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            service.find_other(&q, &|_| true, &go_on),
            Err(LookupError::Cancelled)
        );
        off.join().unwrap();
        assert!(began.elapsed() < Duration::from_millis(600));
        assert_eq!(http.asked().len(), 1);
    }

    #[test]
    fn requests_keep_their_pace() {
        assert!(MUSICBRAINZ_GAP >= Duration::from_secs(1));
        assert!(DEEZER_GAP >= Duration::from_millis(200));
        let http = Arc::new(RecordingHttp::default().answer(
            "https://musicbrainz.org/",
            Ok(fixture("musicbrainz_recording.json")),
        ));
        let service = MusicBrainzService::new(
            Arc::clone(&http) as Arc<dyn Http>,
            Duration::from_millis(60),
        );
        let began = Instant::now();
        // A search and two covers that are not there: three requests, two gaps.
        let _ = service.find(&query(Some("A"), Some("T"), "x"));
        assert_eq!(http.asked().len(), 3);
        assert!(began.elapsed() >= Duration::from_millis(120));
    }

    #[test]
    fn the_user_agent_names_the_app() {
        assert_eq!(
            USER_AGENT,
            format!(
                "OpenJockey/{} ( https://github.com/Roxxas96/open-jockey )",
                env!("CARGO_PKG_VERSION")
            )
        );
    }

    #[test]
    fn the_real_client_refuses_plain_http_without_a_request() {
        // Refused before any connection is made: no network in this test.
        assert!(matches!(
            UreqHttp::new().get("http://example.invalid/x.jpg", IMAGE_LIMIT),
            Err(HttpError::Refused(_))
        ));
    }
}
