use super::*;
use crate::config::store::{Persistence, Store};
use std::os::unix::fs::{PermissionsExt, symlink};

mod confirmation;
mod saves;
mod sources;

pub struct Fixture {
    pub directory: tempfile::TempDir,
    pub source: PathBuf,
    pub target: PathBuf,
    pub tool: Chezmoi,
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

impl Fixture {
    pub fn new() -> Self {
        Self::named("readonly_réglages file.toml", "réglages file.toml")
    }

    fn named(source_name: &str, target_name: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join(source_name);
        let target = directory.path().join(target_name);
        let text = "# mine\n[media]\nimage_preview = true # keep\n[accounts.other.filters]\nkeywords = [\"old\"] # keep array\n";
        fs::write(&source, text).unwrap();
        fs::write(&target, text).unwrap();
        fs::set_permissions(&source, Permissions::from_mode(0o640)).unwrap();
        fs::set_permissions(&target, Permissions::from_mode(0o444)).unwrap();
        let program = directory.path().join("managed-config-probe");
        // This fake implements the documented source-path and target-only apply subset.
        let script = format!(
            r#"#!/bin/sh
source={source}
target={target}
root={root}
action=
for arg in "$@"; do
    case "$arg" in source-path|apply) action="$arg";; esac
    last="$arg"
done
test "$last" = "$target" || exit 2
test ! -f "$root/slow" || sleep 10
case "$action" in
source-path) test ! -f "$root/unmanaged" || exit 1
    if test -f "$root/source-override"; then cat "$root/source-override"; else printf '%s\n' "$source"; fi;;
apply) test ! -f "$root/fail" || exit 1
    printf '%s\n' "$@" > "$root/applied-args"
    test ! -f "$root/noop" || exit 0
    cp "$source" "$target.next" || exit 1
    if test -f "$root/writable"; then chmod 600 "$target.next"; else chmod 444 "$target.next"; fi
    mv "$target.next" "$target";;
*) exit 2;;
esac
"#,
            source = quote(&source),
            target = quote(&target),
            root = quote(directory.path())
        );
        fs::write(&program, script).unwrap();
        fs::set_permissions(&program, Permissions::from_mode(0o700)).unwrap();
        let tool = Chezmoi {
            program,
            args: Vec::new(),
            timeout: Duration::from_secs(2),
        };
        Self {
            directory,
            source,
            target,
            tool,
        }
    }

    pub fn store(&self) -> Store {
        Store::open(Some(&self.target)).unwrap()
    }

    pub fn change(&self, store: &mut Store) {
        store
            .change(
                SETTINGS
                    .iter()
                    .find(|f| f.key == "media.image_preview")
                    .unwrap(),
                toml::Value::Boolean(false),
            )
            .unwrap();
    }

    pub fn plan(&self, store: &Store) -> Plan {
        store
            .managed_request()
            .unwrap()
            .discover(self.tool.clone())
            .unwrap()
            .unwrap()
    }

    fn save(&self, store: &mut Store) {
        let plan = self.plan(store);
        store.managed_finished(plan.verification(), plan.apply());
    }
}
