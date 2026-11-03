//! Loading, ordering, errors and save/load round trips of collections.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::path::{Path, PathBuf};

use snapreq_collection::save::{save_collection, save_folder, save_request, to_toml};
use snapreq_collection::{BodySpec, Collection, Error, Item, RequestFile};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/basic")
}

fn temp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("snapreq-collection-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn loads_items_in_order() {
    let (c, warnings) = Collection::load(fixture()).unwrap();
    assert_eq!(c.file.name, "Basic API");

    let keys: Vec<String> = c.requests().into_iter().map(|(k, _)| k).collect();
    assert_eq!(
        keys,
        [
            "health",
            "users/list-users",
            "users/create-user",
            "users/upload-avatar",
            "zeta"
        ]
    );
    // `environments/` and hidden directories are not folders.
    assert!(
        c.items
            .iter()
            .all(|i| i.key() != "environments" && i.key() != ".hidden")
    );
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0].message.contains("`missing-item`"),
        "{}",
        warnings[0]
    );
}

#[test]
fn request_contents() {
    let (c, _) = Collection::load(fixture()).unwrap();
    let list = c.find("users/list-users").unwrap();
    assert_eq!(list.name(), "List users");
    let query: Vec<(&str, &str, bool)> = list
        .file
        .query
        .0
        .iter()
        .map(|kv| (kv.name.as_str(), kv.value.as_str(), kv.enabled))
        .collect();
    assert_eq!(
        query,
        [
            ("page", "2", true),
            ("tag", "a", true),
            ("tag", "b", true),
            ("draft", "true", false)
        ]
    );

    let create = c.find("users/create-user.toml").unwrap();
    assert_eq!(create.file.method(), "POST");
    let Some(BodySpec::Json {
        content: Some(json),
        ..
    }) = &create.file.body
    else {
        panic!("expected a JSON body");
    };
    assert!(json.starts_with("{\n  \"name\": \"snap\""), "{json}");

    assert_eq!(c.find("health").unwrap().name(), "health");
    assert_eq!(c.find("zeta").unwrap().file.method(), "HEAD");
    assert!(c.find("nope").is_none());

    let folders = c.folders_of("users/create-user");
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0].file.name.as_deref(), Some("Users"));
}

#[test]
fn errors_point_at_the_file_and_line() {
    let dir = temp_dir("errors");
    std::fs::write(dir.join("snapreq.toml"), "format = 1\nname = \"X\"\n").unwrap();
    std::fs::write(
        dir.join("bad.toml"),
        "url = \"http://a\"\n\nmethd = \"GET\"\n",
    )
    .unwrap();

    let err = Collection::load(&dir).unwrap_err();
    let Error::Invalid {
        path,
        location,
        message,
    } = &err
    else {
        panic!("{err:?}");
    };
    assert!(path.ends_with("bad.toml"));
    assert_eq!(*location, Some((3, 1)));
    assert!(message.contains("unknown field `methd`"), "{message}");
    assert!(err.to_string().contains("bad.toml:3:1: "), "{err}");

    // Semantic rules are checked too.
    std::fs::write(
        dir.join("bad.toml"),
        "url = \"http://a\"\n[body]\ntype = \"json\"\ncontent = \"{}\"\nfile = \"b.json\"\n",
    )
    .unwrap();
    let err = Collection::load(&dir).unwrap_err().to_string();
    assert!(err.contains("not both"), "{err}");
}

#[test]
fn format_versions() {
    let dir = temp_dir("format");
    std::fs::write(dir.join("snapreq.toml"), "format = 2\nname = \"Future\"\n").unwrap();
    assert!(matches!(
        Collection::load(&dir).unwrap_err(),
        Error::UnsupportedFormat {
            found: 2,
            supported: 1,
            ..
        }
    ));

    std::fs::write(dir.join("snapreq.toml"), "name = \"No format\"\n").unwrap();
    assert!(
        Collection::load(&dir)
            .unwrap_err()
            .to_string()
            .contains("missing field `format`")
    );

    let empty = temp_dir("not-a-collection");
    assert!(
        Collection::load(&empty)
            .unwrap_err()
            .to_string()
            .contains("not a SnapRequest collection")
    );
}

#[test]
fn saving_and_loading_again_gives_the_same_collection() {
    let (original, _) = Collection::load(fixture()).unwrap();
    let out = temp_dir("roundtrip");

    save_collection(&out.join("snapreq.toml"), &original.file).unwrap();
    fn save_items(items: &[Item], dir: &Path) {
        for item in items {
            match item {
                Item::Request(r) => save_request(&dir.join(&r.path), &r.file).unwrap(),
                Item::Folder(f) => {
                    save_folder(&dir.join(&f.path).join("folder.toml"), &f.file).unwrap();
                    save_items(&f.items, dir);
                }
            }
        }
    }
    save_items(&original.items, &out);

    let (reloaded, _) = Collection::load(&out).unwrap();
    assert_eq!(reloaded.file, original.file);
    assert_eq!(reloaded.items, original.items);
}

#[test]
fn saved_files_read_like_hand_written_ones() {
    let (c, _) = Collection::load(fixture()).unwrap();
    let text = to_toml(&c.find("users/create-user").unwrap().file).unwrap();
    assert!(
        text.starts_with("name = \"Create user\"\nmethod = \"POST\"\nurl = "),
        "{text}"
    );
    assert!(
        text.contains("\n[headers]\nAuthorization = \"Bearer {{token}}\"\n"),
        "{text}"
    );
    assert!(
        text.contains("\n[body]\ntype = \"json\"\ncontent = \"\"\"\n{\n  \"name\": \"snap\",\n"),
        "multi-line bodies stay readable: {text}"
    );
    assert!(
        text.contains("\n[settings]\nfollow_redirects = false\n"),
        "{text}"
    );

    let list = to_toml(&c.find("users/list-users").unwrap().file).unwrap();
    assert!(
        list.contains("[query]\npage = \"2\"\ntag = [\"a\", \"b\"]\ndraft = { value = \"true\", enabled = false }\n"),
        "{list}"
    );

    let upload = to_toml(&c.find("users/upload-avatar").unwrap().file).unwrap();
    assert!(
        upload.contains("[[body.parts]]\nname = \"title\"\nvalue = \"Me\"\n"),
        "{upload}"
    );

    let mut new = RequestFile::new("get", "https://a.test");
    new.name = Some("New".into());
    assert_eq!(
        to_toml(&new).unwrap(),
        "name = \"New\"\nurl = \"https://a.test\"\n"
    );

    new.auth = Some(snapreq_collection::AuthSpec::ApiKey {
        name: "X-Key".into(),
        value: "{{key}}".into(),
        location: snapreq_core::model::ApiKeyLocation::Header,
    });
    let text = to_toml(&new).unwrap();
    assert!(
        text.ends_with("\n[auth]\ntype = \"api_key\"\nname = \"X-Key\"\nvalue = \"{{key}}\"\n"),
        "{text}"
    );
    let back: RequestFile = toml::from_str(&text).unwrap();
    assert_eq!(back, new);
}
