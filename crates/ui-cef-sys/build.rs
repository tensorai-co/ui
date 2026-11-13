//! Build script for `ui-cef-sys`:
//! - Reads `cef.lock` from the crate manifest directory (falling back to workspace root)
//! - Auto-fetches and verifies the pinned Spotify CEF `_minimal` archive using OS `curl` + `tar`
//! - Caches under `$UI_CEF_DIR` or `~/.cache/ui/cef/<version>-<platform>`
//! - Extracts `cef_api_hash` / `cef_version` constants into `OUT_DIR/cef_generated.rs`
//! - Stages runtime assets into `target/{debug,release}` (and builds/stages macOS helper `.app` bundles)

use std::env;
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Default)]
struct PlatformEntry {
    file: String,
    sha1: String,
    sha256: Option<String>,
}

#[derive(Debug, Default)]
struct CefLock {
    version: String,
    base_url: String,
    entries: Vec<(String, PlatformEntry)>,
}

fn resolve_cef_lock(manifest_dir: &Path) -> PathBuf {
    let local = manifest_dir.join("cef.lock");
    if local.exists() {
        return local;
    }
    if let Some(workspace_root) = manifest_dir.parent().and_then(|p| p.parent()) {
        let root_lock = workspace_root.join("cef.lock");
        if root_lock.exists() {
            return root_lock;
        }
    }
    local
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let lock_path = resolve_cef_lock(&manifest_dir);

    println!("cargo:rerun-if-changed={}", lock_path.display());
    println!("cargo:rerun-if-env-changed=UI_CEF_DIR");

    let lock = parse_cef_lock(&lock_path);
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let plat = spotify_platform(&target_os, &target_arch);
    let entry = lock
        .entries
        .iter()
        .find(|(k, _)| k == plat)
        .map(|(_, v)| v)
        .unwrap_or_else(|| panic!("no entry for platform `{plat}` in {}", lock_path.display()));

    let cef_root = resolve_and_fetch_cef(&lock.version, &lock.base_url, plat, entry);
    println!("cargo:rustc-env=UI_CEF_ROOT={}", cef_root.display());

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let api_version = generate_api_hash_rs(&cef_root, &out_dir, &target_os);

    let profile_dir = find_profile_dir(&out_dir);
    stage_runtime_assets(&target_os, &cef_root, &out_dir, &profile_dir, api_version);
    println!(
        "cargo:rustc-env=UI_CEF_PROFILE_DIR={}",
        profile_dir.display()
    );
}

fn spotify_platform(os: &str, arch: &str) -> &'static str {
    match (os, arch) {
        ("macos", "aarch64") => "macosarm64",
        ("macos", "x86_64") => "macosx64",
        ("windows", "x86_64") => "windows64",
        ("windows", "aarch64") => "windowsarm64",
        ("linux", "x86_64") => "linux64",
        ("linux", "aarch64") => "linuxarm64",
        _ => panic!("unsupported target for ui-cef-sys: {os}/{arch}"),
    }
}

fn parse_cef_lock(path: &Path) -> CefLock {
    let content = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let mut lock = CefLock::default();
    let mut current_section: Option<String> = None;
    let mut current_entry = PlatformEntry::default();

    for raw_line in content.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            if let Some(sec) = current_section.take() {
                lock.entries.push((sec, std::mem::take(&mut current_entry)));
            }
            current_section = Some(line[1..line.len() - 1].trim().to_owned());
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let key = k.trim();
            let val = v.trim().trim_matches('"').to_owned();
            match current_section.as_deref() {
                None => match key {
                    "version" => lock.version = val,
                    "base_url" => lock.base_url = val.trim_end_matches('/').to_owned(),
                    _ => {}
                },
                Some(_) => match key {
                    "file" => current_entry.file = val,
                    "sha1" => current_entry.sha1 = val.to_ascii_lowercase(),
                    "sha256" => current_entry.sha256 = Some(val.to_ascii_lowercase()),
                    _ => {}
                },
            }
        }
    }
    if let Some(sec) = current_section {
        lock.entries.push((sec, current_entry));
    }
    assert!(!lock.version.is_empty(), "cef.lock missing `version`");
    assert!(!lock.base_url.is_empty(), "cef.lock missing `base_url`");
    lock
}

fn default_cache_base() -> PathBuf {
    if let Ok(home) = env::var("HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home).join(".cache").join("ui").join("cef");
    }
    if let Ok(local) = env::var("LOCALAPPDATA")
        && !local.is_empty()
    {
        return PathBuf::from(local).join("ui").join("cef");
    }
    env::temp_dir().join("ui-cef-cache")
}

fn resolve_and_fetch_cef(
    version: &str,
    base_url: &str,
    plat: &str,
    entry: &PlatformEntry,
) -> PathBuf {
    if let Ok(override_dir) = env::var("UI_CEF_DIR")
        && !override_dir.is_empty()
    {
        let p = PathBuf::from(&override_dir);
        if p.join("include").join("cef_api_hash.h").exists() {
            return p;
        }
        panic!(
            "UI_CEF_DIR={} does not contain include/cef_api_hash.h",
            p.display()
        );
    }

    let safe_ver = version.replace('+', "_");
    let cache_base = default_cache_base();
    let cef_dir = cache_base.join(format!("{safe_ver}-{plat}"));
    let marker = cef_dir.join(".ui-cef-ok");

    if marker.exists() && cef_dir.join("include").join("cef_api_hash.h").exists() {
        return cef_dir;
    }

    fs::create_dir_all(&cache_base).expect("create ~/.cache/ui/cef directory");
    let archive_path = cache_base.join(&entry.file);
    let part_path = cache_base.join(format!("{}.part", entry.file));
    let url = format!("{base_url}/{}", entry.file);

    if !archive_path.exists() {
        println!("cargo:warning=ui-cef-sys: downloading {url}");
        let curl = if cfg!(windows) { "curl.exe" } else { "curl" };
        let status = Command::new(curl)
            .args(["-fSL", "--retry", "3", "-o"])
            .arg(&part_path)
            .arg(&url)
            .status()
            .unwrap_or_else(|e| panic!("failed to launch `{curl}` to download CEF: {e}"));
        if !status.success() {
            let _ = fs::remove_file(&part_path);
            panic!("failed to download CEF from {url} (exit {status})");
        }
        fs::rename(&part_path, &archive_path).expect("finalize downloaded CEF archive");
    }

    verify_archive_hashes(&archive_path, entry);

    let staging_dir = cache_base.join(format!(".extract-{safe_ver}-{plat}"));
    if staging_dir.exists() {
        let _ = fs::remove_dir_all(&staging_dir);
    }
    fs::create_dir_all(&staging_dir).expect("create CEF extraction staging dir");

    let tar = if cfg!(windows) { "tar.exe" } else { "tar" };
    println!("cargo:warning=ui-cef-sys: extracting {}...", entry.file);
    let status = Command::new(tar)
        .arg("-xjf")
        .arg(&archive_path)
        .arg("--strip-components=1")
        .arg("-C")
        .arg(&staging_dir)
        .status()
        .unwrap_or_else(|e| panic!("failed to run `{tar}`: {e}"));
    if !status.success() {
        let _ = fs::remove_dir_all(&staging_dir);
        panic!(
            "tar failed extracting {} (exit {status})",
            archive_path.display()
        );
    }

    if cef_dir.exists() {
        let _ = fs::remove_dir_all(&cef_dir);
    }
    fs::rename(&staging_dir, &cef_dir).expect("move extracted CEF into cache");
    fs::write(&marker, version).expect("write .ui-cef-ok marker");
    cef_dir
}

fn verify_archive_hashes(path: &Path, entry: &PlatformEntry) {
    let file = File::open(path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let mut sha1 = Sha1::new();
    let mut sha256 = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut buf).expect("read CEF archive");
        if n == 0 {
            break;
        }
        sha1.update(&buf[..n]);
        sha256.update(&buf[..n]);
    }
    let got_sha1 = to_hex(&sha1.finalize());
    if got_sha1 != entry.sha1 {
        let _ = fs::remove_file(path);
        panic!(
            "SHA-1 mismatch for {}: expected {}, got {got_sha1}",
            path.display(),
            entry.sha1
        );
    }
    let got_sha256 = to_hex(&sha256.finalize());
    if let Some(ref exp256) = entry.sha256
        && &got_sha256 != exp256
    {
        let _ = fs::remove_file(path);
        panic!(
            "SHA-256 mismatch for {}: expected {exp256}, got {got_sha256}",
            path.display()
        );
    }
}

fn generate_api_hash_rs(cef_root: &Path, out_dir: &Path, target_os: &str) -> i32 {
    let vers_path = cef_root.join("include").join("cef_api_versions.h");
    let hash_path = cef_root.join("include").join("cef_api_hash.h");
    let ver_path = cef_root.join("include").join("cef_version.h");

    let vers_h = fs::read_to_string(&vers_path).unwrap_or_default();
    let hash_h = fs::read_to_string(&hash_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", hash_path.display()));
    let ver_h = fs::read_to_string(&ver_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", ver_path.display()));

    let api_version = resolve_last_api_version(&vers_h)
        .or_else(|| extract_define_int(&hash_h, "CEF_API_VERSION_LAST"))
        .or_else(|| extract_define_int(&hash_h, "CEF_API_VERSION"))
        .unwrap_or(15600);

    let os_macro = match target_os {
        "macos" => "defined(OS_MAC)",
        "windows" => "defined(OS_WIN)",
        "linux" => "defined(OS_LINUX)",
        _ => "defined(OS_POSIX)",
    };

    let hash_macro = format!("CEF_API_HASH_{api_version}");
    let platform_hash = extract_os_guarded_define(&vers_h, &hash_macro, os_macro)
        .or_else(|| extract_define_str(&hash_h, "CEF_API_HASH_PLATFORM"))
        .or_else(|| extract_define_str(&hash_h, "CEF_API_HASH_UNIVERSAL"))
        .unwrap_or_default();
    let cef_version = extract_define_str(&ver_h, "CEF_VERSION").unwrap_or_default();

    let code = format!(
        "pub const CEF_API_VERSION: i32 = {api_version};\n\
         pub const CEF_API_HASH_UNIVERSAL: &str = {platform_hash:?};\n\
         pub const CEF_API_HASH_PLATFORM: &str = {platform_hash:?};\n\
         pub const CEF_VERSION: &str = {cef_version:?};\n"
    );
    fs::write(out_dir.join("cef_generated.rs"), code).expect("write cef_generated.rs");
    api_version
}

fn resolve_last_api_version(vers_h: &str) -> Option<i32> {
    let mut alias_name: Option<String> = None;
    for line in vers_h.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("#define") {
            let mut parts = rest.split_whitespace();
            if parts.next() == Some("CEF_API_VERSION_LAST")
                && let Some(rhs) = parts.next()
            {
                if let Ok(n) = rhs.parse::<i32>() {
                    return Some(n);
                }
                alias_name = Some(rhs.to_owned());
            }
        }
    }
    alias_name.and_then(|name| extract_define_int(vers_h, &name))
}

fn extract_os_guarded_define(src: &str, macro_name: &str, os_cond: &str) -> Option<String> {
    let mut stack: Vec<(bool, bool)> = Vec::new(); // (is_os_switch, is_active)

    for raw_line in src.lines() {
        let t = raw_line.trim();
        if let Some(cond) = t.strip_prefix("#if") {
            // Note: covers `#if`, `#ifdef`, `#ifndef`
            let is_os =
                cond.contains("OS_WIN") || cond.contains("OS_MAC") || cond.contains("OS_LINUX");
            let active = if is_os { cond.contains(os_cond) } else { true };
            stack.push((is_os, active));
        } else if t.starts_with("#elif") {
            if let Some((is_os, active)) = stack.last_mut()
                && *is_os
            {
                *active = t.contains(os_cond);
            }
        } else if t.starts_with("#else") {
            if let Some((is_os, active)) = stack.last_mut()
                && *is_os
            {
                *active = false;
            }
        } else if t.starts_with("#endif") {
            stack.pop();
        } else if stack.iter().all(|(_, active)| *active)
            && let Some(val) = extract_define_str(t, macro_name)
        {
            return Some(val);
        }
    }
    None
}

fn extract_define_int(src: &str, name: &str) -> Option<i32> {
    for line in src.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("#define") {
            let mut parts = rest.split_whitespace();
            if parts.next() == Some(name)
                && let Some(val) = parts.next()
                && let Ok(parsed) = val.parse::<i32>()
            {
                return Some(parsed);
            }
        }
    }
    None
}

fn extract_define_str(src: &str, name: &str) -> Option<String> {
    for line in src.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("#define") {
            let rest = rest.trim_start();
            if let Some(after) = rest.strip_prefix(name)
                && after.starts_with(char::is_whitespace)
            {
                let val = after.trim().trim_matches('"');
                return Some(val.to_owned());
            }
        }
    }
    None
}

fn find_profile_dir(out_dir: &Path) -> PathBuf {
    // OUT_DIR = <target_profile>/build/<pkg>-<hash>/out
    out_dir.ancestors().nth(3).unwrap_or(out_dir).to_path_buf()
}

fn stage_runtime_assets(
    os: &str,
    cef_root: &Path,
    out_dir: &Path,
    profile_dir: &Path,
    api_version: i32,
) {
    if os == "macos" {
        let src_fw = cef_root
            .join("Release")
            .join("Chromium Embedded Framework.framework");
        let dst_fw = profile_dir.join("Chromium Embedded Framework.framework");
        if src_fw.exists() && !dst_fw.exists() {
            let status = Command::new("ditto")
                .arg(&src_fw)
                .arg(&dst_fw)
                .status()
                .expect("run ditto to stage CEF framework");
            assert!(
                status.success(),
                "ditto failed staging Chromium Embedded Framework.framework"
            );
        }
        stage_macos_helpers(cef_root, out_dir, profile_dir, api_version);
    }
}

fn stage_macos_helpers(cef_root: &Path, out_dir: &Path, profile_dir: &Path, api_version: i32) {
    let helper_c_path = out_dir.join("ui_cef_helper.c");
    let helper_bin_path = out_dir.join("ui-cef-helper");

    let c_changed = fs::read_to_string(&helper_c_path).ok().as_deref() != Some(MACOS_HELPER_C);
    if c_changed {
        fs::write(&helper_c_path, MACOS_HELPER_C).expect("write ui_cef_helper.c");
    }

    if c_changed || !helper_bin_path.exists() {
        let status = Command::new("clang")
            .arg("-std=c11")
            .arg("-O2")
            .arg("-Wall")
            .arg("-Wextra")
            .arg("-Werror")
            .arg("-Wno-unused-parameter")
            .arg("-DOS_MAC=1")
            .arg(format!("-DCEF_API_VERSION={api_version}"))
            .arg("-I")
            .arg(cef_root)
            .arg(&helper_c_path)
            .arg("-o")
            .arg(&helper_bin_path)
            .status()
            .expect("invoke clang for ui-cef-helper");
        assert!(
            status.success(),
            "clang failed compiling macOS CEF subprocess helper"
        );
    }

    let variants = [
        ("ui Helper", "org.ui.cef"),
        ("ui Helper (Alerts)", "org.ui.cef"),
        ("ui Helper (GPU)", "org.ui.cef"),
        ("ui Helper (Plugin)", "org.ui.cef"),
        ("ui Helper (Renderer)", "org.ui.cef"),
    ];
    for (name, bundle_id) in variants {
        let app_dir = profile_dir.join(format!("{name}.app"));
        let contents = app_dir.join("Contents");
        let macos_dir = contents.join("MacOS");
        fs::create_dir_all(&macos_dir).expect("create helper Contents/MacOS");

        let dst_exe = macos_dir.join(name);
        if c_changed || !dst_exe.exists() {
            fs::copy(&helper_bin_path, &dst_exe).expect("copy ui-cef-helper binary");
        }
        let pkginfo = contents.join("PkgInfo");
        if !pkginfo.exists() {
            fs::write(&pkginfo, b"APPL????").expect("write helper PkgInfo");
        }
        let plist_path = contents.join("Info.plist");
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
               <key>CFBundleDevelopmentRegion</key><string>en</string>\n\
               <key>CFBundleDisplayName</key><string>{name}</string>\n\
               <key>CFBundleExecutable</key><string>{name}</string>\n\
               <key>CFBundleIdentifier</key><string>{bundle_id}</string>\n\
               <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>\n\
               <key>CFBundleName</key><string>{name}</string>\n\
               <key>CFBundlePackageType</key><string>APPL</string>\n\
               <key>CFBundleShortVersionString</key><string>1.0.0</string>\n\
               <key>CFBundleVersion</key><string>1</string>\n\
               <key>LSBackgroundOnly</key><true/>\n\
               <key>LSUIElement</key><true/>\n\
               <key>NSSupportsAutomaticGraphicsSwitching</key><true/>\n\
             </dict>\n\
             </plist>\n"
        );
        if fs::read_to_string(&plist_path).ok().as_deref() != Some(&plist) {
            fs::write(&plist_path, plist).expect("write helper Info.plist");
        }
    }
}

const MACOS_HELPER_C: &str = r#"
#include <dlfcn.h>
#include <libgen.h>
#include <mach-o/dyld.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "include/cef_api_hash.h"
#include "include/capi/cef_app_capi.h"
#include "include/capi/cef_browser_capi.h"
#include "include/capi/cef_frame_capi.h"
#include "include/capi/cef_render_process_handler_capi.h"
#include "include/capi/cef_scheme_capi.h"
#include "include/capi/cef_v8_capi.h"
#include "include/capi/cef_values_capi.h"

static const char* (*p_cef_api_hash)(int, int) = NULL;
static int (*p_cef_execute_process)(const cef_main_args_t*, cef_app_t*, void*) = NULL;
static int (*p_cef_string_utf8_to_utf16)(const char*, size_t, cef_string_utf16_t*) = NULL;
static void (*p_cef_string_utf16_clear)(cef_string_utf16_t*) = NULL;
static void (*p_cef_string_userfree_utf16_free)(cef_string_userfree_utf16_t) = NULL;

static char* g_schemes_csv = NULL;

#define CEF_RELEASE(ptr) \
  do { \
    if ((ptr) && ((cef_base_ref_counted_t*)(ptr))->release) { \
      ((cef_base_ref_counted_t*)(ptr))->release((cef_base_ref_counted_t*)(ptr)); \
    } \
  } while (0)

#define CEF_ADD_REF(ptr) \
  do { \
    if ((ptr) && ((cef_base_ref_counted_t*)(ptr))->add_ref) { \
      ((cef_base_ref_counted_t*)(ptr))->add_ref((cef_base_ref_counted_t*)(ptr)); \
    } \
  } while (0)

static void CEF_CALLBACK singleton_add_ref(cef_base_ref_counted_t* self) {}
static int CEF_CALLBACK singleton_release(cef_base_ref_counted_t* self) { return 0; }
static int CEF_CALLBACK singleton_has_one_ref(cef_base_ref_counted_t* self) { return 1; }
static int CEF_CALLBACK singleton_has_at_least_one_ref(cef_base_ref_counted_t* self) { return 1; }

static void init_singleton_base(cef_base_ref_counted_t* base, size_t size) {
  base->size = size;
  base->add_ref = singleton_add_ref;
  base->release = singleton_release;
  base->has_one_ref = singleton_has_one_ref;
  base->has_at_least_one_ref = singleton_has_at_least_one_ref;
}

static cef_string_t utf8_to_cef(const char* s) {
  cef_string_t out = {0};
  if (s && *s) {
    p_cef_string_utf8_to_utf16(s, strlen(s), &out);
  }
  return out;
}

static cef_string_t clone_u16(const cef_string_t* src) {
  cef_string_t out = {0};
  if (src && src->str && src->length > 0) {
    out.str = (char16_t*)malloc(src->length * sizeof(char16_t));
    if (out.str) {
      memcpy(out.str, src->str, src->length * sizeof(char16_t));
      out.length = src->length;
      out.dtor = NULL;
    }
  }
  return out;
}

typedef struct BrowserScripts {
  int browser_id;
  cef_string_t shim_js;
  cef_string_t* init_scripts;
  size_t init_count;
  struct BrowserScripts* next;
} BrowserScripts;

static pthread_mutex_t g_scripts_lock = PTHREAD_MUTEX_INITIALIZER;
static BrowserScripts* g_scripts_head = NULL;
static cef_string_t g_default_shim_js = {0};

static void free_browser_scripts(BrowserScripts* node) {
  if (!node) return;
  free(node->shim_js.str);
  for (size_t i = 0; i < node->init_count; ++i) {
    free(node->init_scripts[i].str);
  }
  free(node->init_scripts);
  free(node);
}

static void store_browser_scripts(int browser_id, struct _cef_dictionary_value_t* extra) {
  if (!extra) return;
  BrowserScripts* node = (BrowserScripts*)calloc(1, sizeof(BrowserScripts));
  if (!node) return;
  node->browser_id = browser_id;

  cef_string_t k_shim = utf8_to_cef("ui_shim");
  if (extra->has_key(extra, &k_shim)) {
    cef_string_userfree_t uf = extra->get_string(extra, &k_shim);
    if (uf) {
      node->shim_js = clone_u16(uf);
      if (g_default_shim_js.length == 0 && uf->length > 0) {
        g_default_shim_js = clone_u16(uf);
      }
      p_cef_string_userfree_utf16_free(uf);
    }
  }
  p_cef_string_utf16_clear(&k_shim);

  cef_string_t k_init = utf8_to_cef("ui_init_scripts");
  if (extra->has_key(extra, &k_init)) {
    struct _cef_list_value_t* list = extra->get_list(extra, &k_init);
    if (list) {
      size_t n = list->get_size(list);
      if (n > 0) {
        node->init_scripts = (cef_string_t*)calloc(n, sizeof(cef_string_t));
        node->init_count = n;
        for (size_t i = 0; i < n; ++i) {
          cef_string_userfree_t uf = list->get_string(list, i);
          if (uf) {
            node->init_scripts[i] = clone_u16(uf);
            p_cef_string_userfree_utf16_free(uf);
          }
        }
      }
      CEF_RELEASE(list);
    }
  }
  p_cef_string_utf16_clear(&k_init);

  pthread_mutex_lock(&g_scripts_lock);
  BrowserScripts** cur = &g_scripts_head;
  while (*cur) {
    if ((*cur)->browser_id == browser_id) {
      BrowserScripts* old = *cur;
      *cur = old->next;
      free_browser_scripts(old);
      break;
    }
    cur = &(*cur)->next;
  }
  node->next = g_scripts_head;
  g_scripts_head = node;
  pthread_mutex_unlock(&g_scripts_lock);
}

static void remove_browser_scripts(int browser_id) {
  pthread_mutex_lock(&g_scripts_lock);
  BrowserScripts** cur = &g_scripts_head;
  while (*cur) {
    if ((*cur)->browser_id == browser_id) {
      BrowserScripts* old = *cur;
      *cur = old->next;
      free_browser_scripts(old);
      break;
    }
    cur = &(*cur)->next;
  }
  pthread_mutex_unlock(&g_scripts_lock);
}

static void eval_u16_in_context(cef_v8_context_t* ctx, const cef_string_t* code, const cef_string_t* url) {
  if (!code || !code->str || code->length == 0) return;
  cef_v8_value_t* retval = NULL;
  cef_v8_exception_t* exc = NULL;
  int ok = ctx->eval(ctx, code, url, 1, &retval, &exc);
  (void)ok;
  CEF_RELEASE(retval);
  CEF_RELEASE(exc);
}

static void CEF_CALLBACK render_on_browser_created(
    struct _cef_render_process_handler_t* self,
    struct _cef_browser_t* browser,
    struct _cef_dictionary_value_t* extra_info) {
  if (browser && extra_info) {
    store_browser_scripts(browser->get_identifier(browser), extra_info);
  }
  CEF_RELEASE(extra_info);
  CEF_RELEASE(browser);
}

static void CEF_CALLBACK render_on_browser_destroyed(
    struct _cef_render_process_handler_t* self,
    struct _cef_browser_t* browser) {
  if (browser) {
    remove_browser_scripts(browser->get_identifier(browser));
    CEF_RELEASE(browser);
  }
}

static void CEF_CALLBACK render_on_context_created(
    struct _cef_render_process_handler_t* self,
    struct _cef_browser_t* browser,
    struct _cef_frame_t* frame,
    struct _cef_v8_context_t* context) {
  if (browser && frame && context && frame->is_main(frame)) {
    int bid = browser->get_identifier(browser);
    if (context->enter(context)) {
      cef_string_t script_url = utf8_to_cef("ui://init.js");
      pthread_mutex_lock(&g_scripts_lock);
      BrowserScripts* found = g_scripts_head;
      while (found && found->browser_id != bid) {
        found = found->next;
      }
      cef_string_t shim = found && found->shim_js.length > 0
          ? clone_u16(&found->shim_js)
          : clone_u16(&g_default_shim_js);
      size_t count = found ? found->init_count : 0;
      cef_string_t* inits = count ? (cef_string_t*)calloc(count, sizeof(cef_string_t)) : NULL;
      for (size_t i = 0; i < count; ++i) {
        inits[i] = clone_u16(&found->init_scripts[i]);
      }
      pthread_mutex_unlock(&g_scripts_lock);

      eval_u16_in_context(context, &shim, &script_url);
      free(shim.str);
      for (size_t i = 0; i < count; ++i) {
        eval_u16_in_context(context, &inits[i], &script_url);
        free(inits[i].str);
      }
      free(inits);
      p_cef_string_utf16_clear(&script_url);
      context->exit(context);
    }
  }
  CEF_RELEASE(context);
  CEF_RELEASE(frame);
  CEF_RELEASE(browser);
}

static cef_render_process_handler_t g_render_handler;

static void register_csv_schemes(cef_scheme_registrar_t* registrar, const char* csv) {
  if (!csv || !*csv) return;
  char* copy = strdup(csv);
  if (!copy) return;
  const int opts = CEF_SCHEME_OPTION_STANDARD |
                   CEF_SCHEME_OPTION_SECURE |
                   CEF_SCHEME_OPTION_CORS_ENABLED |
                   CEF_SCHEME_OPTION_FETCH_ENABLED |
                   CEF_SCHEME_OPTION_CSP_BYPASSING;
  char* tok = copy;
  while (tok && *tok) {
    char* comma = strchr(tok, ',');
    if (comma) *comma = '\0';
    if (*tok) {
      cef_string_t s = utf8_to_cef(tok);
      registrar->add_custom_scheme(registrar, &s, opts);
      p_cef_string_utf16_clear(&s);
    }
    tok = comma ? comma + 1 : NULL;
  }
  free(copy);
}

static void CEF_CALLBACK app_on_register_custom_schemes(
    struct _cef_app_t* self,
    struct _cef_scheme_registrar_t* registrar) {
  const int opts = CEF_SCHEME_OPTION_STANDARD |
                   CEF_SCHEME_OPTION_SECURE |
                   CEF_SCHEME_OPTION_CORS_ENABLED |
                   CEF_SCHEME_OPTION_FETCH_ENABLED |
                   CEF_SCHEME_OPTION_CSP_BYPASSING;
  cef_string_t s_app = utf8_to_cef("app");
  registrar->add_custom_scheme(registrar, &s_app, opts);
  p_cef_string_utf16_clear(&s_app);
  cef_string_t s_ipc = utf8_to_cef("ui-ipc");
  registrar->add_custom_scheme(registrar, &s_ipc, opts);
  p_cef_string_utf16_clear(&s_ipc);
  register_csv_schemes(registrar, g_schemes_csv);
  register_csv_schemes(registrar, getenv("UI_CEF_SCHEMES"));
}

static struct _cef_render_process_handler_t* CEF_CALLBACK app_get_render_process_handler(
    struct _cef_app_t* self) {
  CEF_ADD_REF(&g_render_handler);
  return &g_render_handler;
}

static cef_app_t g_app;

static void* load_libcef(void) {
  const char* env_fw = getenv("UI_CEF_FRAMEWORK_PATH");
  if (env_fw && *env_fw) {
    void* h = dlopen(env_fw, RTLD_LAZY | RTLD_LOCAL | RTLD_FIRST);
    if (h) return h;
  }
  char exe_path[4096];
  uint32_t size = sizeof(exe_path);
  if (_NSGetExecutablePath(exe_path, &size) == 0) {
    char* real = realpath(exe_path, NULL);
    char buf[4096];
    strncpy(buf, real ? real : exe_path, sizeof(buf) - 1);
    buf[sizeof(buf) - 1] = '\0';
    free(real);
    char* dir = dirname(buf);
    if (dir) {
      char candidate[4096];
      snprintf(candidate, sizeof(candidate), "%s/../../../Chromium Embedded Framework.framework/Chromium Embedded Framework", dir);
      void* h = dlopen(candidate, RTLD_LAZY | RTLD_LOCAL | RTLD_FIRST);
      if (h) return h;
      snprintf(candidate, sizeof(candidate), "%s/../../../../Chromium Embedded Framework.framework/Chromium Embedded Framework", dir);
      h = dlopen(candidate, RTLD_LAZY | RTLD_LOCAL | RTLD_FIRST);
      if (h) return h;
    }
  }
  return NULL;
}

int main(int argc, char** argv) {
  for (int i = 1; i < argc; ++i) {
    if (strncmp(argv[i], "--ui-schemes=", 13) == 0) {
      g_schemes_csv = argv[i] + 13;
    }
  }

  void* lib = load_libcef();
  if (!lib) {
    fprintf(stderr, "ui-cef-helper: failed to load Chromium Embedded Framework: %s\n", dlerror());
    return 1;
  }

  p_cef_api_hash = dlsym(lib, "cef_api_hash");
  p_cef_execute_process = dlsym(lib, "cef_execute_process");
  p_cef_string_utf8_to_utf16 = dlsym(lib, "cef_string_utf8_to_utf16");
  p_cef_string_utf16_clear = dlsym(lib, "cef_string_utf16_clear");
  p_cef_string_userfree_utf16_free = dlsym(lib, "cef_string_userfree_utf16_free");

  if (!p_cef_api_hash || !p_cef_execute_process || !p_cef_string_utf8_to_utf16 ||
      !p_cef_string_utf16_clear || !p_cef_string_userfree_utf16_free) {
    fprintf(stderr, "ui-cef-helper: missing required CEF symbols\n");
    return 1;
  }

  p_cef_api_hash(CEF_API_VERSION, 0);

  memset(&g_render_handler, 0, sizeof(g_render_handler));
  init_singleton_base(&g_render_handler.base, sizeof(g_render_handler));
  g_render_handler.on_browser_created = render_on_browser_created;
  g_render_handler.on_browser_destroyed = render_on_browser_destroyed;
  g_render_handler.on_context_created = render_on_context_created;

  memset(&g_app, 0, sizeof(g_app));
  init_singleton_base(&g_app.base, sizeof(g_app));
  g_app.on_register_custom_schemes = app_on_register_custom_schemes;
  g_app.get_render_process_handler = app_get_render_process_handler;

  cef_main_args_t main_args = {argc, argv};
  return p_cef_execute_process(&main_args, &g_app, NULL);
}
"#;

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

// ---------------------------------------------------------------------------
// Zero-dependency SHA-1 and SHA-256 (FIPS 180-4)
// ---------------------------------------------------------------------------

struct Sha1 {
    state: [u32; 5],
    len: u64,
    buf: [u8; 64],
    idx: usize,
}

impl Sha1 {
    fn new() -> Self {
        Self {
            state: [
                0x6745_2301,
                0xEFCD_AB89,
                0x98BA_DCFE,
                0x1032_5476,
                0xC3D2_E1F0,
            ],
            len: 0,
            buf: [0; 64],
            idx: 0,
        }
    }

    fn update(&mut self, data: &[u8]) {
        for &b in data {
            self.buf[self.idx] = b;
            self.idx += 1;
            self.len += 8;
            if self.idx == 64 {
                self.compress();
                self.idx = 0;
            }
        }
    }

    fn finalize(mut self) -> [u8; 20] {
        let total_bits = self.len;
        self.update(&[0x80]);
        while self.idx != 56 {
            self.update(&[0]);
        }
        self.buf[56..64].copy_from_slice(&total_bits.to_be_bytes());
        self.compress();
        let mut out = [0u8; 20];
        for (i, w) in self.state.iter().enumerate() {
            out[i * 4..(i + 1) * 4].copy_from_slice(&w.to_be_bytes());
        }
        out
    }

    fn compress(&mut self) {
        let mut w = [0u32; 80];
        let (chunks, _) = self.buf.as_chunks::<4>();
        for (slot, &chunk) in w[..16].iter_mut().zip(chunks) {
            *slot = u32::from_be_bytes(chunk);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = self.state;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
    }
}

struct Sha256 {
    state: [u32; 8],
    len: u64,
    buf: [u8; 64],
    idx: usize,
}

impl Sha256 {
    fn new() -> Self {
        Self {
            state: [
                0x6a09_e667,
                0xbb67_ae85,
                0x3c6e_f372,
                0xa54f_f53a,
                0x510e_527f,
                0x9b05_688c,
                0x1f83_d9ab,
                0x5be0_cd19,
            ],
            len: 0,
            buf: [0; 64],
            idx: 0,
        }
    }

    fn update(&mut self, data: &[u8]) {
        for &b in data {
            self.buf[self.idx] = b;
            self.idx += 1;
            self.len += 8;
            if self.idx == 64 {
                self.compress();
                self.idx = 0;
            }
        }
    }

    fn finalize(mut self) -> [u8; 32] {
        let total_bits = self.len;
        self.update(&[0x80]);
        while self.idx != 56 {
            self.update(&[0]);
        }
        self.buf[56..64].copy_from_slice(&total_bits.to_be_bytes());
        self.compress();
        let mut out = [0u8; 32];
        for (i, w) in self.state.iter().enumerate() {
            out[i * 4..(i + 1) * 4].copy_from_slice(&w.to_be_bytes());
        }
        out
    }

    fn compress(&mut self) {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut w = [0u32; 64];
        let (chunks, _) = self.buf.as_chunks::<4>();
        for (slot, &chunk) in w[..16].iter_mut().zip(chunks) {
            *slot = u32::from_be_bytes(chunk);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }
}
