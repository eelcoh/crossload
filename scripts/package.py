#!/usr/bin/env python3
"""Build a native binary archive plus its complete, offline-buildable source.

Requires Python 3.11+, the pinned Rust toolchain and native build prerequisites.
Never publishes a release or includes local activation/book/debug data.
"""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
SOURCE_PATHS = (
    "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "mise.toml", "build.rs", "src", "native",
    "tests", "scripts", ".github", ".gitignore", ".containerignore",
    "Containerfile.build", "README.md", "LICENSE", "THIRD-PARTY.md", "NATIVE-OPTIONS.md", "TUI-ARCHITECTURE.md",
)


def run(*args, cwd=ROOT):
    return subprocess.check_output(args, cwd=cwd, text=True)


def archive(source, output):
    with tarfile.open(output, "w:gz", compresslevel=6) as tar:
        for path in [source, *sorted(source.rglob("*"))]:
            if path.is_symlink():
                raise RuntimeError(f"Symlink is not allowed in release sources: {path}")
            info = tar.gettarinfo(str(path), arcname=str(path.relative_to(source.parent)))
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            with path.open("rb") if path.is_file() else contextlib.nullcontext() as data:
                tar.addfile(info, data)


def package(output):
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
    version = manifest["package"]["version"]
    rustc = run("rustc", "-vV")
    triple = next(line[6:] for line in rustc.splitlines() if line.startswith("host: "))
    if platform.system() not in {"Linux", "Darwin"}:
        raise RuntimeError("Release packaging supports Linux and macOS")
    if os.environ.get("CARGO_BUILD_TARGET"):
        raise RuntimeError("Run packaging natively, with CARGO_BUILD_TARGET unset")
    subprocess.run(["cargo", "build", "--release", "--locked"], cwd=ROOT, check=True)
    metadata = json.loads(run("cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"))
    target = Path(metadata["target_directory"])
    binary = target / "release/crossload"
    if platform.system() == "Linux":
        dependencies = run("ldd", str(binary))
        allowed = ("linux-vdso", "libstdc++", "libgcc_s", "libm.so", "libc.so", "libpthread.so", "libdl.so", "librt.so", "/lib", "/usr/lib")
        for line in dependencies.splitlines():
            entry = line.strip()
            if "not found" in entry or (entry and not entry.startswith(allowed)):
                raise RuntimeError(f"Unexpected runtime dependency: {entry}")
    else:
        dependencies = run("otool", "-L", str(binary))
        for line in dependencies.splitlines()[1:]:
            if not line.strip().startswith(("/usr/lib/", "/System/Library/")):
                raise RuntimeError(f"Unexpected runtime dependency: {line.strip()}")
    output.mkdir(parents=True, exist_ok=True)
    stem = f"crossload-{version}-{triple}"
    artifacts = [output / f"{stem}.tar.gz", output / f"{stem}-source.tar.gz", output / f"{stem}.sha256"]
    if any(path.exists() for path in artifacts):
        raise RuntimeError("Package files already exist; use a new --output directory")
    with tempfile.TemporaryDirectory(prefix="package-", dir=target) as temporary:
        temporary = Path(temporary)
        source = temporary / f"{stem}-source"
        source.mkdir()
        for name in SOURCE_PATHS:
            original = ROOT / name
            if original.is_dir():
                shutil.copytree(original, source / name, ignore=shutil.ignore_patterns("__pycache__", "*.pyc"), symlinks=True)
            else:
                shutil.copy2(original, source / name)
        vendor = source / "vendor"
        vendoring = subprocess.run(["cargo", "vendor", "--locked", str(vendor)], cwd=ROOT, text=True, capture_output=True)
        if vendoring.returncode:
            raise RuntimeError(vendoring.stderr)
        config = vendoring.stdout
        (source / ".cargo").mkdir()
        config = re.sub(r'^directory = ".*"$', 'directory = "vendor"', config, flags=re.MULTILINE)
        if tomllib.loads(config).get("source", {}).get("vendored-sources", {}).get("directory") != "vendor":
            raise RuntimeError("Cargo did not emit the vendored source configuration")
        (source / ".cargo/config.toml").write_text(config)
        # Check that every locked dependency is actually resolvable without a
        # registry or Git checkout. A clean offline compile is verified separately.
        empty_cache = temporary / "empty-cargo-home"
        empty_cache.mkdir()
        resolved = subprocess.run(["cargo", "metadata", "--offline", "--locked", "--format-version", "1"], cwd=source, env={**os.environ, "CARGO_HOME": str(empty_cache)}, text=True, capture_output=True)
        if resolved.returncode:
            raise RuntimeError(resolved.stderr)
        rebuild = """Rebuild this source archive
===========================
Install the toolchain in rust-toolchain.toml and C/C++ compiler, Make, CMake,
Perl, and pkg-config (see README.md). All Cargo and native library sources are
included; edit native/vendor or vendor as needed to rebuild with modified
libraries. Then run from this directory:

    cargo build --release --locked --offline

The binary is target/release/crossload. Toolchain and system build tools must be
installed beforehand; rustup cannot download them offline. Modifying files in a
Cargo-vendored crate also requires updating that crate's .cargo-checksum.json
file hashes. Native vendored libraries can be edited directly.
"""
        (source / "REBUILD.txt").write_text(rebuild)
        release = temporary / stem
        release.mkdir()
        shutil.copy2(binary, release / "crossload")
        shutil.copy2(target / "release/xteink", release / "xteink")
        for name in ["README.md", "LICENSE", "THIRD-PARTY.md", "NATIVE-OPTIONS.md"]:
            shutil.copy2(ROOT / name, release / name)
        # Native sources/notices travel with the binary as well, so references
        # from THIRD-PARTY.md resolve. Cargo dependency notices are collected below.
        shutil.copytree(ROOT / "native", release / "native", symlinks=True)
        notices = release / "licenses"
        inventory = []
        for crate in sorted(vendor.iterdir()):
            manifest_path = crate / "Cargo.toml"
            if not manifest_path.exists():
                continue
            crate_manifest = tomllib.loads(manifest_path.read_text())["package"]
            inventory.append({key: crate_manifest.get(key) for key in ["name", "version", "license", "license-file", "repository"]})
            for notice in crate.rglob("*"):
                if notice.is_file() and re.match(r"^(LICENSE|COPYING|NOTICE|COPYRIGHT)([._-].*)?$", notice.name, re.IGNORECASE):
                    destination = notices / crate.name / notice.relative_to(crate)
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(notice, destination)
            if crate_manifest.get("license-file"):
                notice = crate / crate_manifest["license-file"]
                if notice.exists() and notice.resolve().is_relative_to(crate.resolve()):
                    destination = notices / crate.name / notice.relative_to(crate)
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(notice, destination)
        notices.mkdir(exist_ok=True)
        (notices / "dependencies.json").write_text(json.dumps(inventory, indent=2) + "\n")
        try:
            revision = run("git", "rev-parse", "HEAD").strip()
            dirty = bool(run("git", "status", "--porcelain").strip())
        except subprocess.CalledProcessError:
            revision, dirty = None, None
        (release / "BUILD.json").write_text(json.dumps({"version": version, "target": triple, "build_system": platform.platform(), "libc": platform.libc_ver(), "git_revision": revision, "working_tree_modified": dirty, "rustc": rustc, "runtime_dependencies": dependencies, "source_archive": artifacts[1].name}, indent=2) + "\n")
        (release / "INSTALL.txt").write_text(f"""crossload {version} ({triple})

Run ./crossload --help, or install in your user bin directory:

    mkdir -p "$HOME/.local/bin"
    install -m 755 crossload "$HOME/.local/bin/crossload"

No Rust, Python, Calibre, or separately installed ebook libraries are required
at runtime. Standard OS C/C++ runtime libraries are required. This macOS binary,
if applicable, is not signed/notarized; local system policy may restrict it.

The corresponding source archive is {artifacts[1].name}.
Distribute it alongside this binary archive; it includes all dependency sources
and offline rebuild instructions, including replacement of LGPL libraries.
See LICENSE, THIRD-PARTY.md, native/, and licenses/ for notices.
""")
        archive(source, artifacts[1])
        archive(release, artifacts[0])
    checksums = []
    for path in artifacts[:2]:
        with path.open("rb") as stream:
            checksums.append(f"{hashlib.file_digest(stream, 'sha256').hexdigest()}  {path.name}\n")
    artifacts[2].write_text("".join(checksums))
    for path in artifacts:
        print(path)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    package(args.output.resolve())
