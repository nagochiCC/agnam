#!/usr/bin/env python3
"""Fail closed: validate extracted payload, runtime resolution and source manifest."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile

from package import (SYSTEM, CSS_PATCH, LOCK, digest, require, run, output, write_json,
                     verify_inputs, verify_dsc, extract, app_css)


def environment(root, state):
    env = {k: v for k, v in os.environ.items() if k not in
           ['DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'XDG_RUNTIME_DIR',
            'LD_PRELOAD', 'LD_LIBRARY_PATH', 'GTK_PATH', 'GTK_THEME', 'GDK_BACKEND']}
    # Isolate validation only; AppRun itself does not alter users' XDG save paths.
    env.update(LD_LIBRARY_PATH=str(root / 'usr/lib') + ':' + str(root / 'usr/lib/gdk-pixbuf-2.0/2.10.0/loaders'),
        XDG_DATA_DIRS=str(root / 'usr/share') + ':/usr/local/share:/usr/share',
        GSETTINGS_SCHEMA_DIR=str(root / 'usr/share/glib-2.0/schemas'),
        GIO_MODULE_DIR=str(root / 'usr/lib/gio/modules'),
        GDK_PIXBUF_MODULEDIR=str(root / 'usr/lib/gdk-pixbuf-2.0/2.10.0/loaders'),
        GDK_PIXBUF_MODULE_FILE=str(root / 'usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache'),
        FONTCONFIG_PATH=str(root / 'etc/fonts'), FONTCONFIG_FILE=str(root / 'etc/fonts/fonts.conf'),
        HOME=str(state / 'home'), XDG_CONFIG_HOME=str(state / 'config'), XDG_CACHE_HOME=str(state / 'cache'),
        XDG_DATA_HOME=str(state / 'data'))
    return env


def payload(root, report):
    env = environment(root, report.parent / 'test-state')
    require(os.access(root / 'AppRun', os.X_OK), 'AppRun not executable')
    require(os.access(root / 'usr/bin/agnam', os.X_OK), 'Agnam not executable')
    require((root / 'io.github.nagochicc.agnam.svg').is_file(), 'Icon missing')
    run('desktop-file-validate', root / 'io.github.nagochicc.agnam.desktop')
    require(not list(root.glob('usr/share/locale/**/*.mo')), 'Unexpected translation catalogs')
    elfs = []
    inherited = json.loads((root / 'usr/share/doc/agnam/distribution-build-paths.json').read_text())
    for p in sorted(root.rglob('*')):
        if p.is_symlink():
            require(p.resolve().is_relative_to(root) and p.exists(), f'External/broken symlink: {p}')
        if not p.is_file():
            continue
        with p.open('rb') as f:
            magic = f.read(4)
        if magic != b'\x7fELF':
            continue
        require(not SYSTEM.match(p.name), f'Forbidden system library bundled: {p}')
        require(not re.search(r'(?i)(jbig|lzo)', p.name), f'Forbidden library bundled: {p}')
        dynamic = output('readelf', '-d', p)
        needed = re.findall(r'\(NEEDED\).*?\[(.*?)\]', dynamic)
        require(not any(re.search(r'(?i)(jbig|lzo)', n) for n in needed), f'JBIG/LZO dependency: {p}')
        paths = re.findall(r'(?:RPATH|RUNPATH).*?\[(.*?)\]', dynamic)
        require(all(x == '$ORIGIN' or x.startswith('$ORIGIN/') for v in paths for x in v.split(':')), f'Nonrelative RPATH: {p}: {paths}')
        data = p.read_bytes()
        require(not any(x in data for x in [b'/recipe/', b'/home/runner/', b'/home/pc/']), f'Absolute host build path: {p}')
        paths_in_binary = [x.decode('utf-8', errors='strict') for x in re.findall(rb'/build/[A-Za-z0-9_./+=:@~,-]+', data)]
        allowed = inherited.get(str(p.relative_to(root)), {}).get('paths', [])
        require(paths_in_binary == allowed, f'Absolute local build path: {p}: {paths_in_binary}')
        resolution = output('ldd', p, env=env)
        require('not found' not in resolution, f'Unresolved dependency: {p}')
        for name, value in re.findall(r'^\s*(\S+) => (/[^(\n ]+)', resolution, re.M):
            if not SYSTEM.match(Path(name).name):
                require(Path(value).is_relative_to(root), f'Unexpected host dependency: {name} -> {value}')
        versions = output('readelf', '--version-info', p)
        versions = versions.split('Version needs section')[-1] if 'Version needs section' in versions else ''
        minimum = {}
        for family in ['GLIBC', 'GLIBCXX']:
            values = re.findall(r'Name: ' + family + r'_([\d.]+)', versions)
            if values:
                minimum[family] = max(values, key=lambda s: tuple(map(int, s.split('.'))))
        if 'GLIBC' in minimum:
            require(tuple(map(int, minimum['GLIBC'].split('.'))) <= (2, 39), f'GLIBC newer than Ubuntu 24.04: {p}')
        elfs.append({'file': str(p.relative_to(root)), 'needed': needed, 'rpath': paths,
                     'required_symbols': minimum, 'sha256': digest(p), 'resolution': resolution})
    require(elfs, 'No payload ELF')
    required = ['libgtk-4.so.1', 'libadwaita-1.so.0', 'libcairo.so.2', 'libtiff.so.6', 'libturbojpeg.so.0',
                'libgdk_pixbuf-2.0.so.0', 'librsvg-2.so.2', 'libwebp.so.7', 'libstdc++.so.6', 'libfontconfig.so.1']
    require(all((root / 'usr/lib' / p).is_file() for p in required), 'Required library missing')
    doc = root / 'usr/share/doc/agnam'
    require(json.loads((doc / 'agnam-patches.json').read_text()) == CSS_PATCH, 'CSS patch record mismatch')
    for relative in ['fonts.conf', 'conf.d/50-user.conf', 'conf.d/51-local.conf', 'local.conf', 'conf.avail/51-local.conf']:
        require((root / 'etc/fonts' / relative).is_file(), f'Fontconfig configuration missing: {relative}')
    require(not list(root.glob('usr/share/fonts/**/*')), 'Host font files bundled')
    for p in ['LICENSE', 'THIRD_PARTY_NOTICES.md', 'SOURCE_CODE.md', 'third-party/UnRAR/acknow.txt',
              'third-party/UnRAR/blake2sp.cpp', 'third-party/rust/COPYRIGHT-library.html']:
        require((doc / p).is_file(), f'Notice missing: {p}')
    for name in ['crc-catalog', 'nt-time']:
        require(list((doc / 'third-party/cargo').glob(name + '*/LICENSES/*')), f'Nested LICENSES missing: {name}')
    run(root / 'AppRun', '--help', env=env)
    write_json(report, {'elf': elfs, 'jbig_needed': [], 'lzo_needed': [], 'gui_verified': False, 'distribution_build_paths': inherited})


def probe(root, executable, fixtures, report):
    env = environment(root, report.parent / 'test-state')
    # Validation fixture only: proves the normal XDG user font directory is scanned.
    # Font files never enter AppDir or the candidate.
    font = next(iter(sorted(Path('/usr/share/fonts').rglob('*.ttf'))), None)
    require(font is not None, 'Validation environment requires a system test font')
    user_fonts = Path(env['XDG_DATA_HOME']) / 'fonts'
    user_fonts.mkdir(parents=True, exist_ok=True)
    user_font = user_fonts / 'appimage-validation.ttf'
    shutil.copyfile(font, user_font)
    args = ['--css', root / 'usr/share/doc/agnam/APP_CSS.css',
            *sorted(fixtures.iterdir()), root / 'io.github.nagochicc.agnam.svg']
    try:
        result = subprocess.run([str(executable), *map(str, args)], env=env, capture_output=True, text=True)
        require(result.returncode == 0, f'Non-GUI probe failed:\n{result.stdout}{result.stderr}')
        # LD_DEBUG proves GTK/Adwaita, loaders and custom libs load from AppDir.
        trace = subprocess.run([str(executable), *map(str, args)], env=env | {'LD_DEBUG': 'libs'}, capture_output=True, text=True)
        require(trace.returncode == 0, f'Loader probe failed:\n{trace.stdout}{trace.stderr}')
    finally:
        user_font.unlink()  # Keep font bytes out of validation/diagnostic artifacts too.
    require('Fontconfig warning' not in result.stderr and 'Theme parser error' not in result.stderr,
            f'Font/CSS warning: {result.stderr}')
    for name in ['libgtk-4.so.1', 'libadwaita-1.so.0', 'libcairo.so.2', 'libturbojpeg.so.0', 'libfontconfig.so.1',
                 'libpixbufloader-svg.so', 'libpixbufloader-webp.so', 'libdconfsettings.so']:
        require(any('calling init:' in line and str(root) in line and name in line for line in trace.stderr.splitlines()), f'Bundled module not initialized: {name}')
    require('Fontconfig warning' not in trace.stderr and 'Theme parser error' not in trace.stderr,
            'Font/CSS warning under loader tracing')
    report.write_text(result.stdout + '\n' + result.stderr + trace.stderr)
    print(result.stdout, end='')


def image(artifact, w):
    with artifact.open('rb') as f:
        require(f.read(11)[8:11] == b'AI\x02', 'Not AppImage Type 2')
    require(os.access(artifact, os.X_OK), 'AppImage not executable')
    extracted = w / 'extraction'
    extracted.mkdir()
    env = environment(w / 'AppDir', w / 'image-test-state')
    run(artifact, '--appimage-extract', cwd=extracted, env=env, stdout=subprocess.DEVNULL)
    root = extracted / 'squashfs-root'
    for p in (w / 'AppDir').rglob('*'):
        other = root / p.relative_to(w / 'AppDir')
        if p.is_symlink():
            require(other.is_symlink() and os.readlink(p) == os.readlink(other), f'Extracted symlink mismatch: {p}')
        elif p.is_file():
            require(digest(p) == digest(other), f'Extracted file mismatch: {p}')
    run(artifact, '--appimage-extract-and-run', '--help', env=env)
    payload(root, w / 'validation/elf.json')
    probe(root, w / 'nogui-probe', w / 'fixtures', w / 'validation/ubuntu-probe.log')


def source(archive, w):
    target = w / 'source-extraction'
    target.mkdir()
    with tarfile.open(archive) as t:
        t.extractall(target, filter='data')
    s = target / 'corresponding-source'
    listed = set()
    for line in (s / 'SHA256SUMS').read_text().splitlines():
        sha, rel = line.split('  ', 1)
        p = s / rel
        require(p.resolve().is_relative_to(s) and p.is_file(), f'Manifest path missing/unsafe: {rel}')
        require(digest(p) == sha, f'Manifest hash mismatch: {rel}')
        listed.add(rel)
    require(listed == {str(p.relative_to(s)) for p in s.rglob('*') if p.is_file() and p.name != 'SHA256SUMS'}, 'Source manifest coverage mismatch')
    for component in json.loads((s / 'ubuntu-source-manifest.json').read_text()):
        descriptors = [s / p for p in component['files'] if p.endswith('.dsc')]
        require(len(descriptors) == 1, 'Missing source descriptor in source manifest')
        verify_dsc(descriptors[0])
        text = descriptors[0].read_text()
        require(re.search(r'^Source: ' + re.escape(component['source']) + r'$', text, re.M), 'Source package identity mismatch')
        require(re.search(r'^Version: ' + re.escape(component['version']) + r'$', text, re.M), 'Source package version mismatch')
    verify_inputs(s / 'inputs')
    original = s / 'inputs' / LOCK['sources']['agnam']['file']
    require(digest(original) == LOCK['sources']['agnam']['sha256'], 'Original tag archive mismatch')
    require(json.loads((s / 'agnam-patches.json').read_text()) == CSS_PATCH, 'Source patch record mismatch')
    app = extract(original, w / 'patched-source-check')
    # Use the recipe/patch actually distributed in this archive, not the checkout.
    run('python3', '-c', 'import package; from pathlib import Path; import sys; package.apply_agnam_patch(Path(sys.argv[1]))',
        app, cwd=s / 'packaging/appimage')
    require(app_css(app) == (w / 'AppDir/usr/share/doc/agnam/APP_CSS.css').read_text(), 'Rebuilt CSS differs from payload')
    require((s / 'patches/libfuse/mount.c.diff').is_file(), 'AppImage libfuse patch missing')
    # Offline build from the archive, never reuse the original objects or prefixes.
    run('bash', s / 'packaging/appimage/native.sh', s / 'inputs', w / 'source-rebuild', 'all')
    runtime = w / 'source-rebuild/runtime-x86_64'
    run(runtime, '--appimage-version')
    # Use the rebuilt/relinked runtime to extract and run the original candidate payload.
    offset = int(output(w / 'candidate.AppImage', '--appimage-offset').strip())
    replaced = w / 'relinked-runtime.AppImage'
    with replaced.open('wb') as f:
        with runtime.open('rb') as r:
            shutil.copyfileobj(r, f)
        with (w / 'candidate.AppImage').open('rb') as original:
            original.seek(offset)
            shutil.copyfileobj(original, f)
    replaced.chmod(0o755)
    run(replaced, '--appimage-extract-and-run', '--help', env=environment(w / 'AppDir', w / 'relink-test-state'))
    write_json(w / 'validation/source.json', {'manifest_files': len(listed), 'all_hashes_verified': True,
        'tiff_jbig_off_rebuilt': True, 'cairo_lzo_disabled_rebuilt': True, 'turbo_simd_rebuilt': True,
        'fontconfig_relative_templates_rebuilt': True, 'agnam_css_patch_reapplied': True,
        'runtime_offline_rebuilt': True, 'relinked_runtime_extract_and_run': True,
        'runtime_sha256': digest(runtime)})


def main():
    p = argparse.ArgumentParser()
    p.add_argument('command', choices=['payload', 'image', 'source', 'probe'])
    p.add_argument('paths', type=Path, nargs='+')
    a = p.parse_args()
    globals()[a.command](*[x.resolve() for x in a.paths])


if __name__ == '__main__':
    main()
