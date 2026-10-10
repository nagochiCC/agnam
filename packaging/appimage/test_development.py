"""Development trust boundaries, source fidelity and release separation."""
import copy
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import development
import package
import validate


SHA = '1' * 40
CSS = 'const APP_CSS: &str = r#"' + '\n'.join(
    'color: var(--sidebar-' + color + '-color);' for color in development.CSS_COLORS) + '"#;\n'


class DevelopmentChecks(unittest.TestCase):
    def make_archive(self, work):
        inputs = work / 'input'
        inputs.mkdir()
        archive = inputs / ('agnam-' + SHA + '.tar.gz')
        files = {'src/app/mod.rs': CSS.encode(), 'Cargo.toml': b'[package]\nname="agnam"\nversion="0.9.0"\n',
                 'Cargo.lock': b'[[package]]\nname="agnam"\nversion="0.9.0"\n',
                 'LICENSE': b'MIT', 'executable.sh': b'#!/bin/sh\n'}
        with tarfile.open(archive, 'w:gz') as tar:
            for rel, data in files.items():
                member = tarfile.TarInfo('agnam-' + SHA + '/' + rel)
                member.size = len(data)
                member.mode = 0o755 if rel == 'executable.sh' else 0o644
                tar.addfile(member, io.BytesIO(data))
        authority = {'distribution': 'development', 'repository': development.REPOSITORY,
            'ref': 'refs/heads/develop', 'commit': SHA, 'workflow_commit': SHA,
            'file': archive.name, 'sha256': package.digest(archive),
            'files': development.archive_manifest(archive), 'source_date_epoch': 1}
        package.write_json(work / 'source-authority.json', authority)
        return authority, archive

    def test_archive_checksum_commit_and_manifest_fail_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            authority, archive = self.make_archive(work)
            self.assertEqual(development.verify_archive(work / 'input', authority), archive)
            for change in [{'sha256': '0' * 64}, {'commit': 'other'}, {'workflow_commit': '2' * 40},
                           {'file': '../source.tar.gz'}, {'repository': 'other/repository'}, {'ref': 'refs/heads/main'}]:
                with self.assertRaises(RuntimeError):
                    development.verify_archive(work / 'input', authority | change)
            damaged = copy.deepcopy(authority)
            damaged['files']['Cargo.lock']['blob'] = '0' * 40
            with self.assertRaisesRegex(RuntimeError, 'manifest mismatch'):
                development.verify_archive(work / 'input', damaged)
            archive.write_bytes(archive.read_bytes() + b'changed')
            with self.assertRaisesRegex(RuntimeError, 'checksum mismatch'):
                development.verify_archive(work / 'input', authority)

    def test_unsafe_duplicate_and_unsupported_archive_entries_are_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            archive = Path(tmp) / 'source.tar'
            for name, kind, link, duplicate in [
                ('app/../outside', tarfile.REGTYPE, '', False),
                ('/app/outside', tarfile.REGTYPE, '', False),
                ('app/link', tarfile.SYMTYPE, '/outside', False),
                ('app/link', tarfile.LNKTYPE, 'app/file', False),
                ('app/file', tarfile.REGTYPE, '', True),
            ]:
                with tarfile.open(archive, 'w') as tar:
                    member = tarfile.TarInfo(name)
                    member.type = kind
                    member.linkname = link
                    tar.addfile(member)
                    if duplicate:
                        tar.addfile(member)
                with self.assertRaises(RuntimeError):
                    development.archive_manifest(archive)

    def test_source_validation_requires_explicit_matching_distribution(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for mode in [False, True]:
                source = root / ('fixture-' + str(mode)) / 'corresponding-source'
                source.mkdir(parents=True)
                package.write_json(source / 'source-authority.json', {'distribution': 'release' if mode else 'development'})
                package.write_json(source / 'ubuntu-source-manifest.json', [])
                (source / 'SHA256SUMS').write_text(''.join(
                    package.digest(p) + '  ' + p.name + '\n' for p in source.iterdir() if p.is_file()))
                archive = root / ('fixture-' + str(mode) + '.tar')
                with tarfile.open(archive, 'w') as tar:
                    tar.add(source, arcname='corresponding-source')
                work = root / ('build-' + str(mode))
                work.mkdir()
                with patch.object(validate, 'verify_inputs'), patch.object(validate, 'run') as run:
                    with self.assertRaisesRegex(RuntimeError, 'Source distribution mode mismatch'):
                        validate.source(archive, work, development=mode)
                    run.assert_not_called()

    def test_css_compatibility_is_separate_and_reproducible(self):
        with tempfile.TemporaryDirectory() as tmp:
            app = Path(tmp)
            target = app / development.CSS_TARGET
            target.parent.mkdir(parents=True)
            target.write_text(CSS)
            record = development.apply_css(app)
            self.assertEqual(len(record['replacements']), 3)
            self.assertNotIn('var(', package.app_css(app, record))
            self.assertNotEqual(record['original_sha256'], record['patched_sha256'])
            no_patch = development.apply_css(app)
            self.assertEqual(no_patch['replacements'], [])
            self.assertEqual(no_patch['original_sha256'], no_patch['patched_sha256'])
            target.write_text(CSS.replace('var(--sidebar-bg-color)', '@unexpected'))
            with self.assertRaisesRegex(RuntimeError, 'Unexpected development sidebar CSS'):
                development.apply_css(app)
            target.write_text(CSS + 'irrelevant outside CSS')
            self.assertEqual(len(development.apply_css(app)['replacements']), 3)
            target.write_text(CSS.replace('color: var(--sidebar-bg-color);', 'color: var(--sidebar-bg-color); color: var(--unknown);'))
            with self.assertRaisesRegex(RuntimeError, 'Unsupported development CSS'):
                development.apply_css(app)

    def test_snapshot_matches_commit_tree_and_checked_out_recipe(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            fixture = work / 'fixture'
            fixture.mkdir()
            authority, archive = self.make_archive(fixture)
            app = package.extract(archive, work / 'checkout')
            tree = ''.join(f"{v['mode']} blob {v['blob']}\t{p}\0" for p, v in authority['files'].items())
            env = {'GITHUB_SHA': SHA, 'GITHUB_REPOSITORY': development.REPOSITORY,
                'GITHUB_REF': 'refs/heads/develop', 'GITHUB_EVENT_NAME': 'workflow_dispatch',
                'GITHUB_WORKFLOW_SHA': SHA, 'GITHUB_RUN_ID': '123', 'GITHUB_RUN_ATTEMPT': '2'}

            def git_output(*args, **kwargs):
                return {'rev-parse': SHA + '\n', 'status': '', 'ls-tree': tree, 'show': '1\n'}[args[1]]

            def git_archive(*args, **kwargs):
                with tarfile.open(archive) as tar:
                    with tarfile.open(fileobj=kwargs['stdout'], mode='w') as dest:
                        for member in tar.getmembers():
                            dest.addfile(member, tar.extractfile(member))

            with patch.dict(os.environ, env), patch.object(development, 'output', side_effect=git_output), \
                    patch.object(development, 'run', side_effect=git_archive):
                development.snapshot(app, work / 'snapshot')
                actual = json.loads((work / 'snapshot/source-authority.json').read_text())
                self.assertEqual(actual['commit'], SHA)
                self.assertEqual(actual['files'], authority['files'])
                self.assertEqual(actual['run_url'], 'https://github.com/nagochiCC/agnam/actions/runs/123')
                development.verify_archive(work / 'snapshot/input', actual)
                with patch.dict(os.environ, {'GITHUB_SHA': '2' * 40, 'GITHUB_WORKFLOW_SHA': '2' * 40}):
                    with self.assertRaisesRegex(RuntimeError, 'Checkout/Actions SHA mismatch'):
                        development.snapshot(app, work / 'wrong-sha')
                with patch.dict(os.environ, {'GITHUB_REF': 'refs/heads/main'}):
                    with self.assertRaisesRegex(RuntimeError, 'Only manual develop'):
                        development.snapshot(app, work / 'wrong-ref')
                with patch.object(development, 'output', side_effect=lambda *a, **k: ' M src/app/mod.rs' if a[1] == 'status' else git_output(*a, **k)):
                    with self.assertRaisesRegex(RuntimeError, 'Tracked checkout modifications'):
                        development.snapshot(app, work / 'dirty')
                with patch.object(development, 'output', side_effect=lambda *a, **k: tree + '100644 blob ' + SHA + '\tmissing\0' if a[1] == 'ls-tree' else git_output(*a, **k)):
                    with self.assertRaisesRegex(RuntimeError, 'Archive/commit tree mismatch'):
                        development.snapshot(app, work / 'incomplete')
                (app / 'Cargo.lock').write_text('changed')
                with self.assertRaisesRegex(RuntimeError, 'Source file mismatch'):
                    development.snapshot(app, work / 'modified')

    def test_corresponding_source_and_binary_are_bound_to_compiled_commit(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            authority, archive = self.make_archive(work)
            app = package.extract(archive, work / 'agnam-source')
            patch_record = development.apply_css(app)
            package.write_json(work / 'agnam-patches.json', patch_record)
            (app / '.cargo').mkdir()
            (app / '.cargo/config.toml').write_text('vendor configuration')
            (work / 'vendor').mkdir()
            doc = work / 'AppDir/usr/share/doc/agnam'
            doc.mkdir(parents=True)
            binary = work / 'target/release/agnam'
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b'compiled development executable')
            package.copy(binary, work / 'AppDir/usr/bin/agnam')
            (doc / 'APP_CSS.css').write_text(package.app_css(app, patch_record))
            package.copy(work / 'agnam-patches.json', doc / 'agnam-patches.json')
            development.record_build(work, app, doc)
            development.verify_payload(work / 'AppDir')
            record = json.loads((work / 'development-build.json').read_text())
            self.assertEqual(record['commit'], SHA)
            self.assertEqual(record['application_version'], '0.9.0')
            (work / 'input').rename(work / 'inputs')
            rebuilt = development.check_source(work, work)
            self.assertEqual((rebuilt / 'Cargo.lock').read_bytes(), (app / 'Cargo.lock').read_bytes())
            damaged = record | {'commit': '2' * 40}
            package.write_json(work / 'development-build.json', damaged)
            with self.assertRaisesRegex(RuntimeError, 'Compiled source/commit mismatch'):
                development.check_source(work, work / 'wrong-commit')
            (work / 'AppDir/usr/bin/agnam').write_bytes(b'other executable')
            with self.assertRaisesRegex(RuntimeError, 'executable mismatch'):
                development.verify_payload(work / 'AppDir')
            (app / 'Cargo.lock').write_text('changed')
            with self.assertRaisesRegex(RuntimeError, 'Source file mismatch'):
                development.record_build(work, app, doc)

    def test_release_rust_path_keeps_original_authority_and_patch(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            inputs = work / 'input'
            inputs.mkdir()
            record = {'tag': 'v0.9.0', 'commit': package.LOCK['agnam_commit'],
                      'sha256': package.LOCK['sources']['agnam']['sha256']}
            package.write_json(work / 'source-authority.json', record)
            with patch.object(package, 'digest', return_value=record['sha256']), \
                    patch.object(package, 'install_rust', return_value=work / 'tools'), \
                    patch.object(package, 'extract', return_value=work / 'app'), \
                    patch.object(package, 'apply_agnam_patch') as css_patch, \
                    patch.object(package, 'vendor_rust') as vendor, \
                    patch.object(package.tomllib, 'loads', return_value={'package': {'version': '0.9.0'}}), \
                    patch.object(Path, 'read_text', return_value=json.dumps(record)):
                package.rust(inputs)
                css_patch.assert_called_once_with(work / 'app')
                vendor.assert_called_once()
                with patch.object(Path, 'read_text', return_value=json.dumps(record | {'commit': SHA})):
                    with self.assertRaisesRegex(RuntimeError, 'Source authority record mismatch'):
                        package.rust(inputs)

    def test_workflow_keeps_development_and_release_outputs_separate(self):
        recipe = package.RECIPE
        release = (recipe / 'build.sh').read_text()
        dev = (recipe / 'build-development.sh').read_text()
        workflow = (recipe.parent.parent / '.github/workflows/appimage-development.yml').read_text()
        self.assertIn('package.py" authority', release)
        self.assertNotIn('Agnam-development-', release)
        self.assertNotIn('Agnam-0.9.0-', dev)
        self.assertNotIn('package.py" authority', dev)
        self.assertIn('development.py" rust', dev)
        self.assertIn('retention-days: 30', workflow)
        self.assertIn('ref: ${{ github.sha }}', workflow)
        self.assertNotIn('inputs:', workflow)
        self.assertNotIn('contents: write', workflow)
        self.assertNotIn('gh release', workflow)
        self.assertLess(workflow.index('Validate on Debian 13'), workflow.index('Upload validated development build'))

    def test_distribution_names_and_checksum_roundtrip(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            for name in ['candidate.AppImage', 'candidate-source.tar.xz', 'binary-provenance.json',
                         'file-manifest.json', 'ubuntu-packages.json', 'source-authority.json', 'development-build.json']:
                (work / name).write_bytes(name.encode())
            (work / 'validation').mkdir()
            script = (package.RECIPE / 'build-development.sh').read_text()
            subprocess.run(['bash', '-eu', '-c', script[script.index('mkdir dist\n'):]], cwd=work,
                           check=True, capture_output=True, text=True)
            dist = work / 'dist'
            self.assertFalse(list(dist.glob('Agnam-0.9.0-*')))
            for name, original in [('Agnam-development-x86_64.AppImage', 'candidate.AppImage'),
                                   ('Agnam-development-AppImage-corresponding-source.tar.xz', 'candidate-source.tar.xz')]:
                self.assertEqual((dist / name).read_bytes(), (work / original).read_bytes())
                subprocess.run(['sha256sum', '-c', name + '.sha256'], cwd=dist, check=True, capture_output=True)
                (dist / name).write_bytes(b'corrupted')
                result = subprocess.run(['sha256sum', '-c', name + '.sha256'], cwd=dist, capture_output=True)
                self.assertNotEqual(result.returncode, 0)


if __name__ == '__main__':
    unittest.main()
