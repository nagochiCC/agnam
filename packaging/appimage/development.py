#!/usr/bin/env python3
"""Development inputs come exclusively from the verified Actions checkout."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import tarfile
import tomllib

import package
from package import digest, require, run, output, write_json

REPOSITORY = 'nagochiCC/agnam'
CSS_TARGET = 'src/app/mod.rs'
CSS_COLORS = ('bg', 'fg', 'backdrop')


def blob_hash(data):
    return hashlib.sha1(b'blob ' + str(len(data)).encode() + b'\0' + data).hexdigest()


def archive_manifest(archive):
    files = {}
    with tarfile.open(archive) as tar:
        members = tar.getmembers()
        roots = {PurePosixPath(m.name).parts[0] for m in members}
        require(len(roots) == 1, 'Unexpected development archive layout')
        for member in members:
            path = PurePosixPath(member.name)
            require(not path.is_absolute() and '..' not in path.parts, 'Unsafe development source path')
            if member.isdir():
                continue
            require(len(path.parts) > 1 and (member.isfile() or member.issym()), 'Unsupported source entry')
            rel = str(PurePosixPath(*path.parts[1:]))
            require(rel not in files and '.git' not in path.parts, 'Duplicate/Git metadata in source')
            if member.issym():
                require(not PurePosixPath(member.linkname).is_absolute(), 'Absolute source symlink')
                data = member.linkname.encode()
                mode = '120000'
            else:
                data = tar.extractfile(member).read()
                mode = '100755' if member.mode & 0o111 else '100644'
            files[rel] = {'mode': mode, 'blob': blob_hash(data), 'sha256': hashlib.sha256(data).hexdigest()}
    require(files, 'Empty development source')
    return files


def snapshot(checkout, work):
    sha = os.environ['GITHUB_SHA']
    require(re.fullmatch(r'[0-9a-f]{40}', sha), 'Invalid Actions commit SHA')
    require(os.environ['GITHUB_REPOSITORY'] == REPOSITORY, 'Unexpected repository')
    require(os.environ['GITHUB_REF'] == 'refs/heads/develop'
            and os.environ['GITHUB_EVENT_NAME'] == 'workflow_dispatch', 'Only manual develop builds are allowed')
    require(os.environ['GITHUB_WORKFLOW_SHA'] == sha, 'Workflow/checkout commit mismatch')
    for key in ['GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT']:
        require(re.fullmatch(r'[1-9][0-9]*', os.environ[key]), 'Invalid Actions run identity')
    require(output('git', 'rev-parse', 'HEAD', cwd=checkout).strip() == sha, 'Checkout/Actions SHA mismatch')
    require(not output('git', 'status', '--porcelain', '--untracked-files=no', cwd=checkout).strip(),
            'Tracked checkout modifications')
    tree = output('git', 'ls-tree', '-r', '-z', sha, cwd=checkout)
    expected = {}
    for entry in tree.split('\0'):
        if not entry:
            continue
        meta, name = entry.split('\t', 1)
        mode, kind, blob = meta.split()
        require(kind == 'blob' and mode in ['100644', '100755', '120000'], 'Submodules/unsupported source entries')
        expected[name] = {'mode': mode, 'blob': blob}
    epoch = int(output('git', 'show', '-s', '--format=%ct', sha, cwd=checkout).strip())
    require(epoch > 0, 'Invalid source date epoch')
    (work / 'input').mkdir(parents=True, exist_ok=False)
    archive = work / 'input' / ('agnam-' + sha + '.tar.gz')
    with archive.open('wb') as raw, gzip.GzipFile(filename='', fileobj=raw, mode='wb', mtime=0) as gz:
        # Write a plain tar first because subprocess stdout requires a real fd.
        tar_path = work / 'checkout.tar'
        with tar_path.open('wb') as tar:
            run('git', 'archive', '--format=tar', '--prefix=agnam-' + sha + '/', sha, cwd=checkout, stdout=tar)
        with tar_path.open('rb') as tar:
            shutil.copyfileobj(tar, gz)
    tar_path.unlink()
    files = archive_manifest(archive)
    require({p: {k: v[k] for k in ['mode', 'blob']} for p, v in files.items()} == expected,
            'Archive/commit tree mismatch (export attributes are unsupported)')
    # Verify the files used to run the recipe also match the immutable commit.
    verify_tree(checkout, files)
    run_id = os.environ['GITHUB_RUN_ID']
    write_json(work / 'source-authority.json', {
        'distribution': 'development', 'repository': REPOSITORY, 'ref': 'refs/heads/develop',
        'commit': sha, 'tree': output('git', 'rev-parse', sha + '^{tree}', cwd=checkout).strip(),
        'file': archive.name, 'sha256': digest(archive), 'files': files,
        'source_date_epoch': epoch, 'workflow_commit': os.environ['GITHUB_WORKFLOW_SHA'],
        'workflow': '.github/workflows/appimage-development.yml',
        'run_id': run_id, 'run_attempt': os.environ['GITHUB_RUN_ATTEMPT'],
        'run_url': 'https://github.com/' + REPOSITORY + '/actions/runs/' + run_id,
    })


def verify_tree(app, files):
    for rel, entry in files.items():
        path = app / rel
        require(not PurePosixPath(rel).is_absolute() and '..' not in PurePosixPath(rel).parts,
                'Unsafe source manifest path')
        if entry['mode'] == '120000':
            require(path.is_symlink(), 'Source symlink missing: ' + rel)
            data = os.readlink(path).encode()
        else:
            require(not path.is_symlink() and path.is_file(), 'Source file missing: ' + rel)
            require(path.resolve().is_relative_to(app.resolve()), 'Escaping source path')
            require(bool(path.stat().st_mode & 0o111) == (entry['mode'] == '100755'), 'Source mode mismatch: ' + rel)
            data = path.read_bytes()
        require(hashlib.sha256(data).hexdigest() == entry['sha256'] and blob_hash(data) == entry['blob'],
                'Source file mismatch: ' + rel)


def verify_archive(inputs, authority):
    require(authority['distribution'] == 'development' and authority['repository'] == REPOSITORY
            and authority['ref'] == 'refs/heads/develop', 'Development authority mismatch')
    require(re.fullmatch(r'[0-9a-f]{40}', authority['commit'])
            and authority['workflow_commit'] == authority['commit'], 'Development commit mismatch')
    require(authority['file'] == 'agnam-' + authority['commit'] + '.tar.gz', 'Development archive name mismatch')
    archive = inputs / authority['file']
    require(digest(archive) == authority['sha256'], 'Development archive checksum mismatch')
    require(archive_manifest(archive) == authority['files'], 'Development archive manifest mismatch')
    return archive


def apply_css(app):
    target = app / CSS_TARGET
    original = target.read_text()
    match = re.search(r'const APP_CSS: &str = r#"(.*?)"#;', original, re.S)
    require(match, 'Development APP_CSS missing')
    css = match[1]
    replacements = []
    for color in CSS_COLORS:
        old, new = 'var(--sidebar-' + color + '-color)', '@sidebar_' + color + '_color'
        require(css.count(old) + css.count(new) == 1, 'Unexpected development sidebar CSS: ' + color)
        if old in css:
            css = css.replace(old, new)
            replacements.append({'from': old, 'to': new})
    require('var(' not in css, 'Unsupported development CSS variable')
    record = {'distribution': 'development', 'target': CSS_TARGET,
              'original_sha256': digest(target), 'replacements': replacements}
    target.write_text(original[:match.start(1)] + css + original[match.end(1):])
    record['patched_sha256'] = digest(target)
    return record


def rust(inputs):
    authority = json.loads((inputs.parent / 'source-authority.json').read_text())
    archive = verify_archive(inputs, authority)
    require(os.environ['PACKAGING_COMMIT'] == authority['commit'], 'Packaging/source commit mismatch')
    app = package.extract(archive, inputs.parent / 'agnam-source')
    verify_tree(app, authority['files'])
    # Never allow checkout Cargo configuration to redirect dependency authority.
    require(not (app / '.cargo').exists(), 'Checkout Cargo configuration is unsupported')
    for rel in authority['files']:
        if rel.startswith('packaging/appimage/'):
            require(digest(app / rel) == digest(package.RECIPE / rel.removeprefix('packaging/appimage/')),
                    'Mounted recipe/commit mismatch: ' + rel)
    patch = apply_css(app)
    write_json(inputs.parent / 'agnam-patches.json', patch)
    tools = package.install_rust(inputs)
    package.vendor_rust(inputs, tools, app)


def compiled_files(authority, patch):
    files = {p: v.copy() for p, v in authority['files'].items()}
    files[patch['target']]['sha256'] = patch['patched_sha256']
    # The patched blob is verified separately from the original commit blob.
    return files


def record_build(work, app, doc):
    authority = json.loads((work / 'source-authority.json').read_text())
    patch = json.loads((work / 'agnam-patches.json').read_text())
    files = compiled_files(authority, patch)
    files[CSS_TARGET]['blob'] = blob_hash((app / CSS_TARGET).read_bytes())
    verify_tree(app, files)
    package.verify_vendor(app, work / 'vendor')
    record = {'commit': authority['commit'], 'source_archive_sha256': authority['sha256'],
              'application_version': tomllib.loads((app / 'Cargo.toml').read_text())['package']['version'],
              'cargo_lock_sha256': digest(app / 'Cargo.lock'), 'compiled_files': files,
              'build_command': ['cargo', 'build', '--release', '--locked', '--offline'],
              'compiled_binary_sha256': digest(work / 'target/release/agnam'),
              'packaged_binary_sha256': digest(work / 'AppDir/usr/bin/agnam')}
    write_json(work / 'development-build.json', record)
    package.copy(work / 'development-build.json', doc / 'development-build.json')
    package.copy(work / 'source-authority.json', doc / 'source-authority.json')


def verify_payload(root):
    doc = root / 'usr/share/doc/agnam'
    authority = json.loads((doc / 'source-authority.json').read_text())
    record = json.loads((doc / 'development-build.json').read_text())
    patch = json.loads((doc / 'agnam-patches.json').read_text())
    require(authority['distribution'] == patch['distribution'] == 'development', 'Development payload identity mismatch')
    require(authority['repository'] == REPOSITORY and authority['ref'] == 'refs/heads/develop'
            and re.fullmatch(r'[0-9a-f]{40}', authority['commit'])
            and authority['workflow_commit'] == authority['commit'], 'Development payload authority mismatch')
    require(patch['target'] == CSS_TARGET and patch['original_sha256'] == authority['files'][CSS_TARGET]['sha256'],
            'Development payload CSS authority mismatch')
    require(record['commit'] == authority['commit'] and record['source_archive_sha256'] == authority['sha256'],
            'Payload/source commit mismatch')
    require(digest(root / 'usr/bin/agnam') == record['packaged_binary_sha256'], 'Payload/build executable mismatch')


def check_source(source, work):
    authority = json.loads((source / 'source-authority.json').read_text())
    archive = verify_archive(source / 'inputs', authority)
    require(not (source / 'inputs' / package.LOCK['sources']['agnam']['file']).exists(), 'Release input mixed with development')
    app = package.extract(archive, work / 'patched-source-check')
    verify_tree(app, authority['files'])
    expected = json.loads((source / 'agnam-patches.json').read_text())
    actual = apply_css(app)
    require(actual == expected, 'Development CSS patch record mismatch')
    record = json.loads((source / 'development-build.json').read_text())
    files = compiled_files(authority, actual)
    files[CSS_TARGET]['blob'] = blob_hash((app / CSS_TARGET).read_bytes())
    require(files == record['compiled_files'] and record['commit'] == authority['commit'], 'Compiled source/commit mismatch')
    require(digest(app / 'Cargo.lock') == record['cargo_lock_sha256'], 'Compiled lockfile mismatch')
    verify_tree(app, files)
    package.verify_vendor(app, source / 'vendor')
    doc = work / 'AppDir/usr/share/doc/agnam'
    require(record == json.loads((doc / 'development-build.json').read_text()), 'Corresponding source/payload build mismatch')
    require(authority == json.loads((doc / 'source-authority.json').read_text()), 'Corresponding source/payload authority mismatch')
    require(package.app_css(app, actual) == (doc / 'APP_CSS.css').read_text(), 'Rebuilt CSS differs from payload')
    for rel in authority['files']:
        if rel.startswith('packaging/appimage/'):
            require(digest(app / rel) == digest(source / rel), 'Corresponding recipe/commit mismatch')
    (app / '.cargo').mkdir()
    (app / '.cargo/config.toml').write_text('[source.crates-io]\nreplace-with="vendored-sources"\n'
        '[source.vendored-sources]\ndirectory=' + json.dumps(str(source / 'vendor')) + '\n')
    return app


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('command', choices=['snapshot', 'fetch', 'rust', 'appdir', 'sources', 'check-source'])
    parser.add_argument('paths', type=Path, nargs='+')
    args = parser.parse_args()
    paths = [p.resolve() for p in args.paths]
    if args.command == 'fetch':
        for name, entry in package.LOCK['sources'].items():
            if name != 'agnam':
                package.download(entry['url'], paths[0] / entry['file'], entry['sha256'])
    elif args.command in ['appdir', 'sources']:
        getattr(package, args.command)(*paths, development=True)
    else:
        globals()[args.command.replace('-', '_')](*paths)


if __name__ == '__main__':
    main()
