"""Small checks for trust boundaries and notice collection; no build/network needed."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import package


class PackagingChecks(unittest.TestCase):
    def test_apprun_preserves_input_method_theme_and_save_paths(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t)
            shutil.copyfile(package.RECIPE / 'AppRun', root / 'AppRun')
            binary = root / 'usr/bin/agnam'
            binary.parent.mkdir(parents=True)
            keys = ['GTK_IM_MODULE', 'GTK_THEME', 'GTK_PATH', 'DISPLAY',
                    'WAYLAND_DISPLAY', 'XDG_CONFIG_HOME', 'XDG_CACHE_HOME', 'XDG_DATA_HOME',
                    'FONTCONFIG_PATH', 'FONTCONFIG_FILE']
            binary.write_text('#!/usr/bin/env python3\nimport json, os, sys\n'
                              f'print(json.dumps({{"env": {{k: os.environ.get(k) for k in {keys!r}}}, "args": sys.argv[1:]}}))\n')
            binary.chmod(0o755)
            values = {k: '/caller path/' + k for k in keys}
            values.update(GTK_IM_MODULE='fcitx', GTK_THEME='CallerTheme')
            for use_caller_path, use_caller_im in [(True, True), (False, True), (False, False)]:
                env = os.environ | values
                if not use_caller_path:
                    del env['GTK_PATH']
                if not use_caller_im:
                    del env['GTK_IM_MODULE']
                    del env['GTK_THEME']
                result = subprocess.run(['sh', str(root / 'AppRun'), '--help'],
                                        env=env, capture_output=True, text=True, check=True)
                actual = json.loads(result.stdout)
                expected = {k: env.get(k) for k in keys}
                expected['FONTCONFIG_PATH'] = str(root / 'etc/fonts')
                expected['FONTCONFIG_FILE'] = str(root / 'etc/fonts/fonts.conf')
                if Path('/usr/lib/gtk-4.0').is_dir():
                    expected['GTK_PATH'] = (expected['GTK_PATH'] + ':' if expected['GTK_PATH'] else '') + '/usr/lib/gtk-4.0'
                self.assertEqual(actual['env'], expected)
                self.assertEqual(actual['args'], ['--help'])

    def test_css_patch_is_exact_and_fail_closed(self):
        with tempfile.TemporaryDirectory() as t:
            app = Path(t)
            target = app / package.CSS_PATCH['target']
            target.parent.mkdir(parents=True)
            # Use the exact original hunk from the distributed patch, without
            # downloading the tag archive for the unit test.
            lines = (package.RECIPE / package.CSS_PATCH['file']).read_text().splitlines(True)[3:]
            original = '\n' * 146 + ''.join(line[1:] for line in lines if line.startswith((' ', '-')))
            expected = '\n' * 146 + ''.join(line[1:] for line in lines if line.startswith((' ', '+')))
            target.write_text(original)
            data = package.CSS_PATCH | {'original_sha256': package.digest(target),
                'patched_sha256': hashlib.sha256(expected.encode()).hexdigest()}
            with patch.object(package, 'CSS_PATCH', data):
                # Changing any source byte must stop before patch execution.
                target.write_text(original + 'unexpected')
                with self.assertRaisesRegex(RuntimeError, 'Unexpected original'):
                    package.apply_agnam_patch(app)
                target.write_text(original)
                with patch.object(package, 'CSS_PATCH', data | {'sha256': '0' * 64}):
                    with self.assertRaisesRegex(RuntimeError, 'patch hash mismatch'):
                        package.apply_agnam_patch(app)
                target.write_text('\n' + original)
                with patch.object(package, 'CSS_PATCH', data | {'original_sha256': package.digest(target)}):
                    with self.assertRaisesRegex(RuntimeError, 'exact context'):
                        package.apply_agnam_patch(app)
                target.write_text(original)
                package.apply_agnam_patch(app)
                self.assertEqual(target.read_text(), expected)
                with self.assertRaisesRegex(RuntimeError, 'Unexpected original'):
                    package.apply_agnam_patch(app)

    def test_system_boundary(self):
        for name in ['libc.so.6', 'libm.so.6', 'ld-linux-x86-64.so.2', 'libGL.so.1',
                     'libEGL.so.1', 'libdrm.so.2', 'libgbm.so.1', 'libvulkan.so.1', 'libgcc_s.so.1']:
            self.assertTrue(package.SYSTEM.match(name), name)
        for name in ['libglib-2.0.so.0', 'libgobject-2.0.so.0', 'libgtk-4.so.1',
                     'libgraphene-1.0.so.0', 'libturbojpeg.so.0', 'libstdc++.so.6']:
            self.assertFalse(package.SYSTEM.match(name), name)

    def test_download_cache_must_match_hash(self):
        with tempfile.TemporaryDirectory() as t:
            p = Path(t) / 'input'
            p.write_bytes(b'original')
            sha = package.digest(p)
            package.download('https://invalid.example/', p, sha)  # Never contacts server.
            p.write_bytes(b'changed')
            with self.assertRaisesRegex(RuntimeError, 'hash mismatch'):
                package.download('https://invalid.example/', p, sha)

    def test_nested_notices(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t)
            for rel in ['LICENSES/BSD-2-Clause.txt', 'vendor/unrar/acknow.txt',
                        'vendor/zstd/LICENSE', 'README.ijg', 'NOTICE']:
                p = root / rel
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text('original text')
            (root / 'unrelated.c').write_text('code')
            self.assertEqual(len(list(package.license_files(root))), 5)

    def test_fresh_download_must_match_hash(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t)
            source = root / 'source'; source.write_bytes(b'release asset')
            target = root / 'download'
            with self.assertRaisesRegex(RuntimeError, 'Download hash mismatch'):
                package.download(source.as_uri(), target, '0' * 64)
            self.assertFalse(target.exists())
            package.download(source.as_uri(), target, package.digest(source))
            self.assertEqual(target.read_bytes(), source.read_bytes())

    def test_vendor_integrity_and_complete_coverage(self):
        with tempfile.TemporaryDirectory() as t:
            app = Path(t) / 'app'; app.mkdir()
            vendor = Path(t) / 'vendor'; vendor.mkdir()
            crate = vendor / 'a'; crate.mkdir()
            (app / 'Cargo.lock').write_text('[[package]]\nname="a"\nversion="1.0.0"\nchecksum="locked"\n')
            (crate / 'Cargo.toml').write_text('[package]\nname="a"\nversion="1.0.0"\n')
            data = crate / 'lib.rs'; data.write_text('original')
            (crate / '.cargo-checksum.json').write_text(json.dumps({'package': 'locked',
                'files': {'lib.rs': package.digest(data)}}))
            package.verify_vendor(app, vendor)
            data.write_text('modified')
            with self.assertRaisesRegex(RuntimeError, 'Vendor file mismatch'):
                package.verify_vendor(app, vendor)

    def test_source_descriptor_hashes(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t)
            tar = root / 'source.tar.xz'; tar.write_bytes(b'source')
            dsc = root / 'source.dsc'
            dsc.write_text(f'Checksums-Sha256:\n {package.digest(tar)} 6 source.tar.xz\n')
            package.verify_dsc(dsc)
            tar.write_bytes(b'edited')
            with self.assertRaisesRegex(RuntimeError, 'hash mismatch'):
                package.verify_dsc(dsc)

    def test_source_authority_constants(self):
        self.assertEqual(package.LOCK['agnam_commit'], 'c5bddcac92995816f0f0cb212db8b11dadf52c2d')
        self.assertEqual(package.LOCK['sources']['agnam']['sha256'], 'aac6269c7621efe565d1a639f790f60acf3e40bb115f60802a4befa47f6301dc')


if __name__ == '__main__':
    unittest.main()
