use std::fs;
use std::path::{Path, PathBuf};

use interop::{InteropError, VdjPoi, VdjSong, find_sources, read_database, read_playlists};
use tempfile::TempDir;

const SONG: &str = r#"<Song FilePath="D:/Artist - Title.mp3" FileSize="9277127" Flag="268435456">
  <Tags Title="Artist - Title" Bpm="0.821918" Key="Cm" Flag="1" />
  <Infos SongLength="231.784490" LastModified="1794228943" FirstSeen="1782069030" Bitrate="320" Cover="16386" />
  <Scan Version="801" Bpm="0.620703" Phase="0.024240" AltBpm="0.362063" Volume="1.138718" Key="F" AudioSig="..." Flag="32768" />
  <Poi Pos="0.020317" Type="automix" Point="realStart" />
  <Poi Pos="6.765714" Num="1" Color="4294901760" Type="cue" />
  <Poi Name="Break 1" Pos="59.204354" Type="remix" />
  <Poi Name="Test named cue" Pos="86.413311" Num="2" Color="4278190335" Type="cue" />
  <Poi Pos="125.960" Num="3" Color="4278255360" Type="cue" />
  <Poi Pos="128.456145" Num="4" Type="loop" Size="8.0" />
  <Poi Pos="227.660045" Type="automix" Point="realEnd" />
 </Song>
"#;

fn database(songs: &str) -> String {
    format!("<VirtualDJ_Database Version=\"2025\">\n{songs}</VirtualDJ_Database>\n")
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn reads_hotcues_loops_names_and_colours() {
    let dir = TempDir::new().unwrap();
    let db = write(dir.path(), "database.xml", &database(SONG));
    let songs = read_database(&db, Some(Path::new("/tmp/X"))).unwrap();
    assert_eq!(
        songs,
        vec![VdjSong {
            path: PathBuf::from("/tmp/X/Artist - Title.mp3"),
            size: Some(9277127),
            seconds_per_beat: Some(0.620703),
            pois: vec![
                VdjPoi::Hotcue {
                    num: 1,
                    seconds: 6.765714,
                    name: None,
                    color: Some(0xFF0000)
                },
                VdjPoi::Hotcue {
                    num: 2,
                    seconds: 86.413311,
                    name: Some("Test named cue".into()),
                    color: Some(0x0000FF)
                },
                VdjPoi::Hotcue {
                    num: 3,
                    seconds: 125.96,
                    name: None,
                    color: Some(0x00FF00)
                },
                VdjPoi::Loop {
                    num: 4,
                    seconds: 128.456145,
                    beats: 8.0,
                    name: None,
                    color: None
                },
            ],
            other_markers: 3,
            from_drive: true,
        }]
    );
}

#[test]
fn a_local_database_keeps_absolute_paths() {
    let dir = TempDir::new().unwrap();
    let xml = database(
        r#"<Song FilePath="/Users/me/a.mp3" FileSize="5"></Song><Song FilePath="C:/Music/b.mp3"/>"#,
    );
    let db = write(dir.path(), "database.xml", &xml);
    let songs = read_database(&db, None).unwrap();
    assert_eq!(songs[0].path, PathBuf::from("/Users/me/a.mp3"));
    assert_eq!(songs[1].path, PathBuf::from("C:/Music/b.mp3"));
    assert_eq!(songs[1].size, None);
}

#[test]
fn entities_and_accents_decode() {
    let dir = TempDir::new().unwrap();
    let xml = database(
        r#"<Song FilePath="D:/Beyoncé &amp; Jay-Z - Drunk &apos;n&apos; Love.mp3"></Song>"#,
    );
    let db = write(dir.path(), "database.xml", &xml);
    let songs = read_database(&db, Some(Path::new("/v"))).unwrap();
    assert_eq!(
        songs[0].path,
        PathBuf::from("/v/Beyoncé & Jay-Z - Drunk 'n' Love.mp3")
    );
}

#[test]
fn a_truncated_database_is_a_format_error_naming_the_file() {
    let dir = TempDir::new().unwrap();
    let full = database(SONG);
    let cut = &full[..full.find("<Poi Pos=\"6.7").unwrap()];
    let db = write(dir.path(), "database.xml", cut);
    let err = read_database(&db, None).unwrap_err();
    assert!(matches!(err, InteropError::Format { .. }));
    assert!(err.to_string().contains("database.xml"));
}

#[test]
fn a_non_vdj_xml_is_a_format_error() {
    let dir = TempDir::new().unwrap();
    let db = write(
        dir.path(),
        "other.xml",
        "<html><Song FilePath=\"x\"/></html>",
    );
    assert!(matches!(
        read_database(&db, None),
        Err(InteropError::Format { .. })
    ));
    let empty = write(dir.path(), "empty.xml", "");
    assert!(matches!(
        read_database(&empty, None),
        Err(InteropError::Format { .. })
    ));
}

#[test]
fn a_missing_database_is_an_io_error_naming_the_file() {
    let err = read_database(Path::new("/nonexistent-oj/database.xml"), None).unwrap_err();
    assert!(matches!(err, InteropError::Io { .. }));
    assert!(err.to_string().contains("database.xml"));
}

fn folder(songs: &[&str]) -> String {
    let lines: String = songs
        .iter()
        .map(|p| format!("\t<song path=\"{p}\" size=\"1\" idx=\"0\" />\n"))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<VirtualFolder noDuplicates=\"no\">\n{lines}</VirtualFolder>\n"
    )
}

#[test]
fn playlists_keep_their_order_and_follow_the_order_file() {
    let dir = TempDir::new().unwrap();
    let lists = dir.path().join("MyLists");
    write(
        &lists,
        "Alpha.vdjfolder",
        &folder(&["/m/a1.mp3", "/m/a2.mp3"]),
    );
    write(&lists, "Zulu.vdjfolder", &folder(&["/m/z1.mp3"]));
    write(&lists, "Sub/Inner.vdjfolder", &folder(&["/m/i1.mp3"]));
    write(
        &lists,
        "Mixtape.m3u",
        "#EXTM3U\n#EXTINF:100,Artist - Song\nsongs/one.mp3\n\n#EXTINF:5,x\n/abs/two.mp3\n",
    );
    write(&lists, "readme.txt", "ignored");
    let order = write(&lists, "order", "Zulu\nSub/Inner\n");

    let (got, errors) = read_playlists(std::slice::from_ref(&lists), Some(&order)).unwrap();
    assert!(errors.is_empty());
    let names: Vec<&str> = got.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Zulu", "Sub / Inner", "Alpha", "Mixtape"]);
    assert_eq!(
        got[2].paths,
        [PathBuf::from("/m/a1.mp3"), PathBuf::from("/m/a2.mp3")]
    );
    // A root-relative `/abs/two.mp3` resolves against the playlist folder's drive,
    // so on Windows it becomes `C:/abs/two.mp3`. Joining onto the root avoids
    // clippy's `join_absolute_paths` lint.
    let root = lists.ancestors().last().unwrap();
    assert_eq!(
        got[3].paths,
        [lists.join("songs/one.mp3"), root.join("abs/two.mp3")]
    );

    let (unordered, _) = read_playlists(&[lists], None).unwrap();
    assert_eq!(unordered[0].name, "Alpha");
}

#[test]
fn find_sources_looks_in_home_dirs_and_volumes() {
    let dir = TempDir::new().unwrap();
    let home1 = dir.path().join("support/VirtualDJ");
    let home2 = dir.path().join("docs/VirtualDJ");
    let missing = dir.path().join("nothing/VirtualDJ");
    write(&home1, "database.xml", "x");
    write(&home1, "MyLists/order", "A\n");
    write(&home2, "MyLists/B.vdjfolder", "x");
    let vol = dir.path().join("vol");
    write(&vol, "VirtualDJ/database.xml", "x");
    write(&vol, "VirtualDJ/MyLists/C.vdjfolder", "x");
    let empty_vol = dir.path().join("empty-vol");
    fs::create_dir_all(&empty_vol).unwrap();

    let s = find_sources(
        &[home1.clone(), home2.clone(), missing],
        &[vol.clone(), empty_vol],
    );
    assert_eq!(
        s.databases,
        [
            home1.join("database.xml"),
            vol.join("VirtualDJ/database.xml")
        ]
    );
    assert_eq!(
        s.list_dirs,
        [
            home1.join("MyLists"),
            home2.join("MyLists"),
            vol.join("VirtualDJ/MyLists")
        ]
    );
    assert_eq!(s.order, Some(home1.join("MyLists/order")));
}

#[cfg(unix)]
#[test]
fn symlinked_folders_and_files_are_not_followed() {
    let dir = TempDir::new().unwrap();
    let lists = dir.path().join("MyLists");
    write(&lists, "Top.vdjfolder", &folder(&["/m/t.mp3"]));
    write(&lists, "Sub/Inner.vdjfolder", &folder(&["/m/i.mp3"]));
    std::os::unix::fs::symlink("..", lists.join("Sub/up")).unwrap();
    std::os::unix::fs::symlink(&lists, lists.join("self")).unwrap();
    std::os::unix::fs::symlink(lists.join("Top.vdjfolder"), lists.join("Alias.vdjfolder")).unwrap();

    let (got, errors) = read_playlists(&[lists], None).unwrap();
    let names: Vec<&str> = got.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Sub / Inner", "Top"]);
    assert!(errors.is_empty());
}

#[test]
fn a_bad_playlist_is_reported_and_the_rest_are_read() {
    let dir = TempDir::new().unwrap();
    let lists = dir.path().join("MyLists");
    write(&lists, "Good.vdjfolder", &folder(&["/m/g.mp3"]));
    let full = folder(&["/m/b.mp3"]);
    write(&lists, "Bad.vdjfolder", &full[..full.len() - 20]);

    let (got, errors) = read_playlists(&[lists], None).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].name, "Good");
    assert_eq!(errors.len(), 1);
    assert!(errors[0].to_string().contains("Bad.vdjfolder"));
}

#[test]
fn an_unreadable_order_file_is_reported_and_names_sort() {
    let dir = TempDir::new().unwrap();
    let lists = dir.path().join("MyLists");
    write(&lists, "B.vdjfolder", &folder(&[]));
    write(&lists, "A.vdjfolder", &folder(&[]));
    let (got, errors) = read_playlists(
        std::slice::from_ref(&lists),
        Some(&lists.join("missing-order")),
    )
    .unwrap();
    assert_eq!(got[0].name, "A");
    assert_eq!(errors.len(), 1);
}
