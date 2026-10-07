#!/usr/bin/env python3
"""v0.9.0 packaging. Only Python's standard library; every failed check is fatal."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tomllib
import urllib.request

RECIPE = Path(__file__).resolve().parent
LOCK = json.loads((RECIPE / 'inputs.json').read_text())
EPOCH = 1791094136
NATIVE = {'turbo', 'cairo', 'tiff-dsc', 'tiff-orig', 'tiff-debian',
          'fontconfig-dsc', 'fontconfig-orig', 'fontconfig-debian'}
CSS_PATCH = {
    'file': 'patches/agnam-v0.9.0-gtk414-css.patch',
    'sha256': '60b85b9841b64c4e6124113cef4b5100da086cdbff3e6a6ac841909de80ca656',
    'target': 'src/app/mod.rs',
    'original_sha256': 'c6d9301ca7db79f8ef76719679bd19875b6e692a03f4200688165d375c87e8b4',
    'patched_sha256': 'ec11d6219342453f2aa8e1f0540b6068b7c1cb2039958dc7042f2f88ebe2d8c6',
}
RUNTIME = {'runtime', 'fuse', 'musl', 'squashfuse', 'zstd', 'zlib', 'mimalloc'}
SYSTEM = re.compile(r'^(ld-linux|lib(c|m|dl|pthread|rt|resolv|util|anl|gcc_s)\.so|'
                    r'lib(GL|EGL|GLX|OpenGL|GLES|GLdispatch|drm|gbm|vulkan|nvidia)|'
                    r'lib(X11|xcb|wayland-client)\.so)')


def run(*args, **kwargs):
    return subprocess.run([str(x) for x in args], check=True, **kwargs)


def output(*args, **kwargs):
    return subprocess.check_output([str(x) for x in args], text=True, **kwargs)


def digest(p):
    with Path(p).open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def write_json(p, value):
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(json.dumps(value, indent=2, ensure_ascii=False) + '\n')


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def download(url, p, sha):
    if p.exists():
        require(digest(p) == sha, f'Cached input hash mismatch: {p}')
        return
    p.parent.mkdir(parents=True, exist_ok=True)
    tmp = p.with_name(p.name + '.partial')
    with urllib.request.urlopen(url, timeout=120) as src, tmp.open('wb') as dst:
        shutil.copyfileobj(src, dst)
    require(digest(tmp) == sha, f'Download hash mismatch: {url}')
    tmp.rename(p)


def verify_dsc(descriptor):
    text = descriptor.read_text()
    match = re.search(r'^Checksums-Sha256:\n((?: [^\n]+\n)+)', text, re.M)
    require(match, f'Source descriptor lacks SHA-256: {descriptor}')
    seen = set()
    for line in match[1].splitlines():
        sha, size, name = line.split()
        require(re.fullmatch(r'[0-9a-f]{64}', sha) and size.isdigit(), 'Invalid source checksum field')
        require(name == Path(name).name and name not in ['.', '..'] and name not in seen, 'Unsafe/duplicate source filename')
        path = descriptor.parent / name
        require(path.resolve().is_relative_to(descriptor.parent) and path.is_file(), f'Source file missing: {name}')
        require(path.stat().st_size == int(size) and digest(path) == sha, f'Source descriptor hash mismatch: {name}')
        seen.add(name)


def verify_inputs(root, mode='all'):
    names = RUNTIME if mode == 'runtime' else NATIVE if mode == 'libraries' else RUNTIME | NATIVE
    for n in names:
        v = LOCK['sources'][n]
        require(digest(root / v['file']) == v['sha256'], f'Input hash mismatch: {n}')
    if mode != 'runtime':
        verify_dsc(root / LOCK['sources']['tiff-dsc']['file'])
        verify_dsc(root / LOCK['sources']['fontconfig-dsc']['file'])


def fetch(root):
    for v in LOCK['sources'].values():
        download(v['url'], root / v['file'], v['sha256'])


def authority(root):
    url = 'https://api.github.com/repos/nagochiCC/agnam/git/ref/tags/v0.9.0'
    with urllib.request.urlopen(url, timeout=60) as f:
        tag = json.load(f)
    obj = tag['object']
    # Both annotated and lightweight tags are supported, only this fixed tag.
    while obj['type'] == 'tag':
        with urllib.request.urlopen('https://api.github.com/repos/nagochiCC/agnam/git/tags/' + obj['sha'], timeout=60) as f:
            obj = json.load(f)['object']
    require(obj['type'] == 'commit' and obj['sha'] == LOCK['agnam_commit'], 'v0.9.0 tag/commit mismatch')
    src = LOCK['sources']['agnam']
    require(digest(root / src['file']) == src['sha256'], 'v0.9.0 archive mismatch')
    write_json(root.parent / 'source-authority.json', {'tag': 'v0.9.0', 'commit': obj['sha'], **src})


def extract(archive, target):
    target.mkdir(parents=True, exist_ok=False)
    with tarfile.open(archive) as t:
        t.extractall(target, filter='data')
    children = list(target.iterdir())
    require(len(children) == 1 and children[0].is_dir(), f'Unexpected archive layout: {archive}')
    return children[0]


def rust(root):
    authority_record = json.loads((root.parent / 'source-authority.json').read_text())
    require(authority_record['tag'] == 'v0.9.0' and authority_record['commit'] == LOCK['agnam_commit']
            and authority_record['sha256'] == LOCK['sources']['agnam']['sha256'], 'Source authority record mismatch')
    require(digest(root / LOCK['sources']['agnam']['file']) == authority_record['sha256'], 'Agnam archive mismatch')
    tools = root.parent / 'toolchain'
    for name in ['rustc', 'cargo', 'rust-std', 'rust-src', 'rust-docs']:
        v = LOCK['sources'][name]
        require(digest(root / v['file']) == v['sha256'], f'Rust component mismatch: {name}')
        tree = extract(root / v['file'], root.parent / ('install-' + name))
        run('bash', tree / 'install.sh', '--prefix=' + str(tools), '--disable-ldconfig')
    require(output(tools / 'bin/rustc', '--version').startswith('rustc ' + LOCK['rust'] + ' '), 'Rust version mismatch')
    app = extract(root / LOCK['sources']['agnam']['file'], root.parent / 'agnam-source')
    require(tomllib.loads((app / 'Cargo.toml').read_text())['package']['version'] == '0.9.0', 'Agnam version mismatch')
    apply_agnam_patch(app)
    write_json(root.parent / 'agnam-patches.json', CSS_PATCH)
    # Complete locked vendor set includes native code, build crates and other targets.
    env = os.environ | {'PATH': str(tools / 'bin') + ':' + os.environ['PATH'], 'CARGO_HOME': str(root.parent / 'cargo-home')}
    config = output(tools / 'bin/cargo', 'vendor', '--locked', root.parent / 'vendor', cwd=app, env=env)
    (app / '.cargo').mkdir(exist_ok=True)
    (app / '.cargo/config.toml').write_text(config)
    # Verify package and every vendor file against Cargo.lock / cargo vendor checksums.
    verify_vendor(app, root.parent / 'vendor')


def apply_agnam_patch(app):
    patch = RECIPE / CSS_PATCH['file']
    target = app / CSS_PATCH['target']
    require(digest(patch) == CSS_PATCH['sha256'], 'CSS patch hash mismatch')
    require(digest(target) == CSS_PATCH['original_sha256'], 'Unexpected original Agnam CSS source')
    result = output('patch', '--batch', '--forward', '--fuzz=0', '-p1', '-i', patch,
                    cwd=app, env=os.environ | {'LC_ALL': 'C'})
    require('offset' not in result and 'fuzz' not in result, 'CSS patch did not apply at exact context')
    require(digest(target) == CSS_PATCH['patched_sha256'], 'Patched Agnam source hash mismatch')


def app_css(app):
    target = app / CSS_PATCH['target']
    require(digest(target) == CSS_PATCH['patched_sha256'], 'Patched Agnam source hash mismatch')
    return re.search(r'const APP_CSS: &str = r#"(.*?)"#;', target.read_text(), re.S)[1]


def verify_vendor(app, vendor):
    packages = tomllib.loads((app / 'Cargo.lock').read_text())['package']
    seen = set()
    for directory in vendor.iterdir():
        p = tomllib.loads((directory / 'Cargo.toml').read_text())['package']
        lock = next(x for x in packages if x['name'] == p['name'] and x['version'] == p['version'])
        checks = json.loads((directory / '.cargo-checksum.json').read_text())
        require(checks['package'] == lock['checksum'], f'Vendor package mismatch: {directory}')
        for rel, sha in checks['files'].items():
            require(digest(directory / rel) == sha, f'Vendor file mismatch: {directory / rel}')
        seen.add((p['name'], p['version']))
    require(seen == {(p['name'], p['version']) for p in packages if 'checksum' in p}, 'Vendor lockfile coverage mismatch')


def copy(src, dst):
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(src, dst)
    shutil.copymode(src, dst)


def license_files(root):
    for p in root.rglob('*'):
        if p.is_file() and (re.match(r'(?i)^(licen[cs]e|copying|copyright|notice|authors|acknow)', p.name)
                            or any(x.upper() == 'LICENSES' for x in p.relative_to(root).parts)
                            or p.name == 'README.ijg'):
            yield p


def collect_texts(root, dst):
    files = list(license_files(root))
    require(files, f'No license texts: {root}')
    for p in files:
        copy(p, dst / p.relative_to(root))


def package_info(package):
    fields = output('dpkg-query', '-W', '-f=${binary:Package}\t${Version}\t${source:Package}\t${source:Version}', package).split('\t')
    require(len(fields) == 4, f'No package metadata: {package}')
    return dict(zip(['package', 'version', 'source', 'source_version'], fields))


def owner(path):
    # Account for Ubuntu's /usr merge and SONAME symlinks without guessing owners.
    candidates = [path, path.resolve()]
    for candidate in list(candidates):
        text = str(candidate)
        if text.startswith('/usr/lib/'):
            candidates.append(Path(text.removeprefix('/usr')))
        elif text.startswith('/lib/'):
            candidates.append(Path('/usr' + text))
    for candidate in candidates:
        value = subprocess.run(['dpkg-query', '-S', str(candidate)], capture_output=True, text=True)
        if value.returncode == 0:
            break
    require(value.returncode == 0, f'Unowned Ubuntu file: {path}')
    names = {line.rsplit(': ', 1)[0] for line in value.stdout.splitlines()}
    require(len(names) == 1, f'Ambiguous owner: {path}: {names}')
    return names.pop()


def appdir(w):
    a = w / 'AppDir'
    a.mkdir(exist_ok=False)
    src = next((w / 'agnam-source').iterdir())
    prefix = w / 'native/prefix'
    env = os.environ | {'LD_LIBRARY_PATH': str(prefix / 'lib')}
    provenance = {}
    packages = {}
    inherited_build_paths = {}

    def record(source, target, component=None):
        if component == 'fontconfig':
            pkg = owner(Path('/usr/lib/x86_64-linux-gnu/libfontconfig.so.1'))
            info = package_info(pkg)
            require(info['source'] == 'fontconfig' and info['source_version'] == '2.15.0-1.1ubuntu2', 'Fontconfig package mismatch')
            packages[pkg] = info.copy()
            info.update(component='fontconfig', template_dir='conf.avail')
        elif component:
            info = {'component': component}
        else:
            pkg = owner(source)
            info = package_info(pkg)
            packages[pkg] = info
            if source.open('rb').read(4) == b'\x7fELF':
                paths = [x.decode('utf-8', errors='strict') for x in re.findall(rb'/build/[A-Za-z0-9_./+=:@~,-]+', source.read_bytes())]
                if paths:
                    # Reviewed Ubuntu originals: OpenSSL compiler flags and librsvg Rust panic locations.
                    require(info['source'] in ['openssl', 'librsvg'], f'Unreviewed distribution build paths: {source}')
                    inherited_build_paths[str(target.relative_to(a))] = {'paths': paths, 'source': info, 'original_sha256': digest(source)}
        copy(source, target)
        provenance[str(target.relative_to(a))] = dict(info, original=str(source), original_sha256=digest(source), sha256=digest(target))

    def dependency(path, target=None):
        name = path.name
        if SYSTEM.match(Path(name).name):
            return
        target = target or a / 'usr/lib' / name
        if target.exists():
            return
        component = None
        if path.is_relative_to(prefix):
            component = ('turbo' if 'turbojpeg' in name else 'tiff' if name.startswith('libtiff')
                         else 'fontconfig' if name.startswith('libfontconfig') else 'cairo')
        record(path, target, component)
        # Absolute ldd results are used, not guessed SONAME locations.
        listing = output('ldd', path, env=env)
        require('not found' not in listing, f'Unresolved build dependency: {path}\n{listing}')
        for soname, resolved in re.findall(r'^\s*(\S+) => (/[^(\n ]+)', listing, re.M):
            if not SYSTEM.match(Path(soname).name):
                dependency(Path(resolved), a / 'usr/lib' / soname)

    record(w / 'target/release/agnam', a / 'usr/bin/agnam', 'agnam')
    listing = output('ldd', w / 'target/release/agnam', env=env)
    require('not found' not in listing, 'Agnam build dependency missing')
    for name, path in re.findall(r'^\s*(\S+) => (/[^(\n ]+)', listing, re.M):
        if not SYSTEM.match(Path(name).name):
            dependency(Path(path), a / 'usr/lib' / name)
    libs = Path('/usr/lib/x86_64-linux-gnu')
    for name in ['svg', 'webp']:
        dependency(libs / f'gdk-pixbuf-2.0/2.10.0/loaders/libpixbufloader-{name}.so',
                   a / f'usr/lib/gdk-pixbuf-2.0/2.10.0/loaders/libpixbufloader-{name}.so')
    dependency(libs / 'gio/modules/libdconfsettings.so', a / 'usr/lib/gio/modules/libdconfsettings.so')
    # Cairo/TIFF must resolve to the rebuilt copies even for indirect dependencies.
    dependency(prefix / 'lib/libcairo-script-interpreter.so.2')
    for name in ['libcairo.so.2', 'libcairo-gobject.so.2', 'libcairo-script-interpreter.so.2', 'libtiff.so.6', 'libturbojpeg.so.0', 'libfontconfig.so.1']:
        require((a / 'usr/lib' / name).is_file(), f'Missing custom library: {name}')
        require(digest(a / 'usr/lib' / name) == digest(prefix / 'lib' / name), f'Stock library leaked: {name}')
    for p in a.rglob('*'):
        if p.is_file() and p.open('rb').read(4) == b'\x7fELF':
            origin = os.path.relpath(a / 'usr/lib', p.parent)
            run('patchelf', '--set-rpath', '$ORIGIN' + (('/' + origin) if origin != '.' else ''), p)
    # Resource selection follows the verified manual GTK4 AppDir, no GTK plugin.
    for source, relative in [(Path('/etc/fonts/fonts.conf'), 'etc/fonts/fonts.conf'),
                             *[(p, 'etc/fonts/conf.d/' + p.name) for p in sorted(Path('/etc/fonts/conf.d').glob('*.conf'))],
                             *[(p, 'etc/fonts/conf.avail/' + p.name) for p in sorted(Path('/usr/share/fontconfig/conf.avail').glob('*.conf'))]]:
        record(source.resolve(), a / relative)
        info = provenance[relative]
        if info['source'] == 'fontconfig':
            require(info['source_version'] == '2.15.0-1.1ubuntu2', 'Fontconfig config version mismatch')
    # Resolve 51-local.conf inside AppDir, including the separate template scan.
    # Host system customization may use syntax unsupported by this library.
    (a / 'etc/fonts/local.conf').write_text('<?xml version="1.0"?><fontconfig/>\n')
    provenance['etc/fonts/local.conf'] = {'component': 'packaging', 'generated': 'empty system customization'}
    doc = a / 'usr/share/doc/agnam'
    doc.mkdir(parents=True)
    (doc / 'APP_CSS.css').write_text(app_css(src))
    provenance['usr/share/doc/agnam/APP_CSS.css'] = {'component': 'agnam', 'generated_from': CSS_PATCH['target'],
                                                  'patched_source_sha256': CSS_PATCH['patched_sha256']}
    copy(w / 'agnam-patches.json', doc / 'agnam-patches.json')
    for relative in ['usr/share/glib-2.0/schemas', 'usr/share/icons/Adwaita', 'usr/share/mime']:
        for p in sorted(Path('/' + relative).rglob('*')):
            if p.is_file():
                # Generated caches have no dpkg owner; regenerate them below.
                if p.name in ['gschemas.compiled', 'icon-theme.cache'] or relative.endswith('/mime') and p.relative_to(Path('/' + relative)).parts[0] != 'packages':
                    continue
                record(p.resolve(), a / str(p).lstrip('/'))
    run('glib-compile-schemas', a / 'usr/share/glib-2.0/schemas')
    run('gtk-update-icon-cache', '-f', '-t', a / 'usr/share/icons/Adwaita')
    run('update-mime-database', a / 'usr/share/mime')
    cache_dir = a / 'usr/lib/gdk-pixbuf-2.0/2.10.0'
    modules = cache_dir / 'loaders'
    env = os.environ | {'LD_LIBRARY_PATH': str(a / 'usr/lib')}
    cache = output(libs / 'gdk-pixbuf-2.0/gdk-pixbuf-query-loaders', *sorted(modules.glob('*.so')), env=env)
    (cache_dir / 'loaders.cache').write_text('\n'.join(line.replace(str(modules) + '/', '') for line in cache.splitlines() if not line.startswith('#')) + '\n')
    run('gio-querymodules', a / 'usr/lib/gio/modules', env=env)
    appid = 'io.github.nagochicc.agnam'
    for p in [Path(f'data/{appid}.desktop'), Path(f'data/icons/hicolor/scalable/apps/{appid}.svg')]:
        target = a / ('usr/share/applications/' + p.name if p.suffix == '.desktop' else 'usr/share/icons/hicolor/scalable/apps/' + p.name)
        record(src / p, target, 'agnam')
        (a / p.name).symlink_to(target.relative_to(a))
    (a / '.DirIcon').symlink_to(appid + '.svg')
    copy(RECIPE / 'AppRun', a / 'AppRun')
    (a / 'AppRun').chmod(0o755)
    # Compiler support objects: include corresponding GCC source as well.
    for p in [Path(output('gcc', '-print-libgcc-file-name').strip()), Path(output('gcc', '-print-file-name=crtbeginS.o').strip())]:
        pkg = owner(p)
        packages[pkg] = package_info(pkg)
    notices(w, a, src, packages)
    write_json(a / 'usr/share/doc/agnam/distribution-build-paths.json', inherited_build_paths)
    for relative, entry in provenance.items():
        entry['sha256'] = digest(a / relative)
    write_json(a / 'usr/share/doc/agnam/components.json', {'ubuntu': list(packages.values()), 'cargo_manifest': 'third-party/cargo.json', 'custom_inputs': {n: LOCK['sources'][n] for n in sorted(RUNTIME | NATIVE)}})
    write_json(w / 'binary-provenance.json', provenance)
    write_json(w / 'file-manifest.json', {str(p.relative_to(a)): digest(p) for p in sorted(a.rglob('*')) if p.is_file()})
    write_json(w / 'ubuntu-packages.json', list(packages.values()))


def notices(w, a, src, packages):
    doc = a / 'usr/share/doc/agnam'
    third = doc / 'third-party'
    copy(src / 'LICENSE', doc / 'LICENSE')
    shutil.copytree('/usr/share/common-licenses', third / 'common-licenses', symlinks=False)
    for pkg in sorted(packages):
        base = Path('/usr/share/doc') / pkg.split(':')[0]
        require((base / 'copyright').is_file(), f'Missing package copyright: {pkg}')
        copy(base / 'copyright', third / 'ubuntu' / pkg / 'copyright')
    crates = []
    for directory in sorted((w / 'vendor').iterdir()):
        meta = tomllib.loads((directory / 'Cargo.toml').read_text())['package']
        files = list(license_files(directory))
        winapi_target = meta['name'] in ['winapi-i686-pc-windows-gnu', 'winapi-x86_64-pc-windows-gnu'] and meta['version'] == '0.4.0' and meta.get('license') == 'MIT/Apache-2.0'
        # lzma-rust's distributed crate declares Apache-2.0 in Cargo.toml but has no standalone license.
        require(files or winapi_target or meta['name'] == 'lzma-rust' and meta.get('license') == 'Apache-2.0', f'Unreviewed crate without texts: {meta}')
        dest = third / 'cargo' / directory.name
        for p in files:
            copy(p, dest / p.relative_to(directory))
        for name in ['Cargo.toml', 'README.md']:
            if (directory / name).is_file():
                copy(directory / name, dest / name)
        supplement = None
        if not files and winapi_target:
            # These Windows-only import packages refer to their parent winapi license.
            parent = w / 'vendor/winapi'
            require(tomllib.loads((parent / 'Cargo.toml').read_text())['package']['version'] == '0.3.9', 'Unreviewed winapi license source')
            for name in ['LICENSE-MIT', 'LICENSE-APACHE']:
                copy(parent / name, dest / name)
            supplement = 'winapi 0.3.9 original MIT/Apache texts (same repository/author); not linked on Linux'
        elif not files:
            copy(Path('/usr/share/common-licenses/Apache-2.0'), dest / 'LICENSE-Apache-2.0')
            supplement = 'Apache-2.0 text; grant and authors retained in original Cargo.toml/README'
        crates.append({'name': meta['name'], 'version': meta['version'], 'license': meta.get('license'),
                       'texts': [str(p.relative_to(directory)) for p in files], 'supplement': supplement})
    unrar = next(p for p in (w / 'vendor').iterdir() if tomllib.loads((p / 'Cargo.toml').read_text())['package']['name'] == 'unrar_sys')
    native = unrar / 'vendor/unrar'
    for name in ['license.txt', 'acknow.txt', 'blake2sp.cpp', 'version.hpp']:
        copy(native / name, third / 'UnRAR' / name)
    require('Intel' in (third / 'UnRAR/acknow.txt').read_text(), 'Intel CRC32 acknowledgement absent')
    require('BLAKE2' in (third / 'UnRAR/acknow.txt').read_text(), 'BLAKE2 acknowledgement absent')
    for n in ['turbo', 'cairo', 'tiff', 'fontconfig']:
        collect_texts(w / 'native/src' / n, third / n)
    for n in sorted(RUNTIME):
        collect_texts(w / 'runtime/src' / n, third / ('runtime-' + n))
    rustdoc = w / 'toolchain/share/doc/rust'
    require((rustdoc / 'COPYRIGHT-library.html').is_file(), 'Rust library copyright absent')
    copy(rustdoc / 'COPYRIGHT-library.html', third / 'rust/COPYRIGHT-library.html')
    shutil.copytree(rustdoc / 'licenses', third / 'rust/licenses')
    # Ubuntu librsvg's Rust panic locations identify Rust 1.75 as a second stdlib.
    # These are upstream notice texts, not a claim about an unknown compiler Debian revision.
    for n, entry in LOCK['sources'].items():
        if n.startswith('librsvg-stdlib-'):
            require(digest(w / 'input' / entry['file']) == entry['sha256'], 'librsvg stdlib notice hash mismatch')
            copy(w / 'input' / entry['file'], third / 'librsvg-rust-stdlib' / entry['file'])
    write_json(third / 'cargo.json', crates)
    (doc / 'THIRD_PARTY_NOTICES.md').write_text('''# Third-party notices — Agnam 0.9.0 AppImage

Agnam: MIT; see LICENSE. This AppImage contains independently licensed libraries,
artwork, data, Cargo native code and a statically linked AppImage runtime.
The adjacent third-party/ directory preserves original notices, including nested
LICENSES, Ubuntu copyright files, and Rust standard library copyright/licenses. Ubuntu librsvg embeds Rust 1.75
standard library code; its upstream COPYRIGHT / MIT / Apache originals are also
retained separately. The Ubuntu compiler Debian revision is not inferred from
panic locations, and these notice files do not claim to reproduce that compiler.
The generated binary-provenance.json / ubuntu-packages.json in the corresponding
source archive identify the exact package/source versions. cargo.json is a
conservative inventory of the complete lockfile, including build/other-target crates.
Automated collection does not itself establish legal completeness; review originals.

This software is based in part on the work of the Independent JPEG Group.
This software is based in part on the work of the FreeType Team.
TurboJPEG 3.2.0 and Ubuntu libjpeg 2.1.5 have separate original notices.
Cairo 1.18.6: LGPL-2.1 is selected from LGPL-2.1 OR MPL-1.1.
Zstd (Cargo, Ubuntu and runtime): BSD-3-Clause is selected from the dual license.
libstdc++ / GCC support: GPL-3.0 with GCC Runtime Library Exception 3.1;
original copyright and exception are retained in Ubuntu notices and GCC sources.

UnRAR native code is NOT MIT. See third-party/UnRAR/license.txt and acknow.txt.
The original license prohibits development of RAR/WinRAR compatible compressors
and recreation of the proprietary RAR compression algorithm. Agnam uses extraction.
acknow.txt preserves Intel CRC32 BSD-2-Clause, BLAKE2sp and other acknowledgements;
blake2sp.cpp preserves Samuel Neves' original CC0 notice.

Adwaita Icon Theme: GNOME Project, Adwaita Icon Theme 46, https://www.gnome.org/.
Artwork is unmodified. For standard dual-licensed artwork LGPL-3.0 is selected;
individual CC-BY-SA-4.0 works retain their original conditions and attribution.
See Ubuntu adwaita-icon-theme copyright, original source/COPYING/AUTHORS,
and https://creativecommons.org/licenses/by-sa/4.0/ (original terms included).
shared-mime-info is separately distributed GPL data, not linked into Agnam.
GTK/GLib/libadwaita/Pango/GdkPixbuf/librsvg/dconf and other libraries retain
all original grants, individual notices, Ubuntu patches and corresponding source.

AppImage runtime: type2-runtime 8f39b89e2ac31e1640b3d3f7e9a5108e6ce805fa (MIT).
Static dependencies: libfuse 3.15.0 (LGPL-2.1), squashfuse 0.5.2,
musl 1.2.5, Zstd 1.5.6, zlib 1.3.2, mimalloc 2.1.7.
libfuse is modified by the upstream AppImage mount.c.diff; source and patch are
provided with an offline runtime rebuild/relink recipe. See SOURCE_CODE.md.
JBIG-KIT and LZO are excluded from the final payload and its ELF dependencies.
''')
    with (doc / 'THIRD_PARTY_NOTICES.md').open('a') as f:
        f.write('\n## Actual Ubuntu package inventory\n\n| Binary package | Version | Source package/version | Original notice |\n|---|---|---|---|\n')
        for name, info in sorted(packages.items()):
            f.write(f"| {name} | {info['version']} | {info['source']} {info['source_version']} | third-party/ubuntu/{name}/copyright |\n")
        f.write('\nFor libraries offering LGPL/GPL alternatives, LGPL is selected; libcap uses its BSD alternative and FreeType uses FTL. GCC runtime exceptions and separately distributed GPL MIME data retain their original conditions.\n')
    (doc / 'SOURCE_CODE.md').write_text('''# Corresponding source and library replacement

Distribute Agnam-0.9.0-AppImage-corresponding-source.tar.xz alongside this AppImage
at the same download location. It includes the actual sources, Ubuntu orig/debian
archives and .dsc, Cargo vendor set, Rust library source, runtime static sources,
AppImage libfuse patch, packaging/build recipes, manifests and offline rebuild checks.
The upstream Agnam v0.9.0 archive is unchanged. The build applies only the three
navigation rail color substitutions recorded in agnam-patches.json, using
packaging/appimage/patches/agnam-v0.9.0-gtk414-css.patch for GTK 4.14 compatibility.
See packaging/appimage/README.md for the offline application/rebuild procedure.
Fontconfig retains Ubuntu 2.15.0-1.1ubuntu2 source/patches; its template directory
is compiled as relative conf.avail to use the packaged configuration.
Do not distribute this candidate until both artifacts and original notices are reviewed.

Extract with `./Agnam-0.9.0-x86_64.AppImage --appimage-extract`.
Compatible modified shared libraries may replace the corresponding files under
`squashfs-root/usr/lib`; run `squashfs-root/AppRun` afterwards. This distribution
places no restriction on modifications or reverse engineering needed to debug them.
For the statically linked libfuse, see packaging/appimage/README.md and native.sh
in the source archive: modify the fuse source/patch, adjust its recorded checksum,
and rebuild the runtime. Agnam itself need not be relinked for this replacement.
''')


def sources(w):
    s = w / 'corresponding-source'
    s.mkdir(exist_ok=False)
    shutil.copytree(RECIPE, s / 'packaging/appimage', ignore=shutil.ignore_patterns('__pycache__'))
    upstream = s / 'inputs'
    upstream.mkdir()
    for n in sorted(RUNTIME | NATIVE | {'agnam', 'rust-src'} | {n for n in LOCK['sources'] if n.startswith('librsvg-stdlib-')}):
        copy(w / 'input' / LOCK['sources'][n]['file'], upstream / LOCK['sources'][n]['file'])
    shutil.copytree(w / 'vendor', s / 'vendor')
    ubuntu = s / 'ubuntu'
    ubuntu.mkdir()
    groups = {(p['source'], p['source_version']) for p in json.loads((w / 'ubuntu-packages.json').read_text())}
    source_manifest = []
    (w / 'source-check').mkdir(exist_ok=True)
    for name, version in sorted(groups):
        dest = ubuntu / name
        dest.mkdir()
        # APT verifies Sources hashes against the signed fixed snapshot InRelease.
        run('apt-get', 'source', '--download-only', name + '=' + version, cwd=dest)
        dscs = list(dest.glob('*.dsc'))
        require(len(dscs) == 1, f'Unexpected source descriptor: {dest}')
        verify_dsc(dscs[0])
        # Signature provenance is APT's signed Sources index; unpack checks patch application.
        run('dpkg-source', '--no-check', '-x', dscs[0], w / 'source-check' / name)
        source_manifest.append({'source': name, 'version': version, 'files': [str(p.relative_to(s)) for p in sorted(dest.iterdir())]})
    for name in ['binary-provenance.json', 'file-manifest.json', 'ubuntu-packages.json', 'source-authority.json', 'agnam-patches.json']:
        copy(w / name, s / name)
    shutil.copytree(w / 'runtime/logs', s / 'runtime-link-evidence')
    # Preserve exact custom build settings, patches and required extra native notices.
    for directory in ['native/turbo', 'native/tiff', 'native/cairo', 'native/fontconfig']:
        p = w / directory
        for name in ['CMakeCache.txt', 'meson-info/intro-buildoptions.json']:
            if (p / name).is_file():
                copy(p / name, s / 'build-settings' / p.name / Path(name).name)
    for p in (w / 'runtime/src/runtime/patches').rglob('*'):
        if p.is_file():
            copy(p, s / 'patches' / p.relative_to(w / 'runtime/src/runtime/patches'))
    write_json(s / 'ubuntu-source-manifest.json', source_manifest)
    write_json(s / 'build-environment.json', {'ubuntu_image': LOCK['ubuntu_image'], 'snapshot': LOCK['ubuntu_snapshot'],
        'packaging_commit': os.environ.get('PACKAGING_COMMIT', 'local-working-tree'),
        'source_date_epoch': EPOCH, 'rust': output(w / 'toolchain/bin/rustc', '-Vv'),
        'installed_packages': output('dpkg-query', '-W'), 'compiler': output('gcc', '--version'), 'runtime_compiler': output('clang', '--version'), 'kernel': list(os.uname())})
    sums = [f'{digest(p)}  {p.relative_to(s)}' for p in sorted(s.rglob('*')) if p.is_file()]
    (s / 'SHA256SUMS').write_text('\n'.join(sums) + '\n')


def main():
    p = argparse.ArgumentParser()
    p.add_argument('command', choices=['fetch', 'verify-inputs', 'authority', 'rust', 'appdir', 'sources'])
    p.add_argument('directory', type=Path)
    p.add_argument('mode', nargs='?', default='all')
    a = p.parse_args()
    if a.command == 'verify-inputs':
        verify_inputs(a.directory.resolve(), a.mode)
    else:
        globals()[a.command](a.directory.resolve())


if __name__ == '__main__':
    main()
