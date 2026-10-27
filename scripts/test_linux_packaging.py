"""Host-independent regressions for Linux packaging and updater routing."""
import base64
import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


class LinuxPackagingTests(unittest.TestCase):
    def test_package_audit_checks_resolved_closure_and_build_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            tools = root / 'tools'
            tools.mkdir()
            package = root / 'package'
            (package / 'usr/bin').mkdir(parents=True)
            (package / 'usr/lib/unsub').mkdir(parents=True)
            (package / 'usr/share/doc/unsub/licenses').mkdir(parents=True)
            (package / 'usr/share/doc/unsub/licenses/sources.tsv').write_text('pinned sources')
            (package / 'usr/bin/unsub').touch(mode=0o755)
            (package / 'AppRun').touch()
            families = ('avcodec', 'avformat', 'avutil', 'swresample', 'mp3lame', 'opus')
            for family in families:
                for parent in ('usr/lib', 'usr/lib/unsub'):
                    (package / parent / f'lib{family}.so.1').touch()
            stub = r'''#!/usr/bin/env python3
import os, pathlib, sys
root = os.environ['AUDIT_FIXTURE_ROOT']
mode = os.environ.get('AUDIT_FIXTURE_MODE', '')
name = pathlib.Path(sys.argv[0]).name
if name == 'file': print('ELF shared object')
elif name == 'ldd':
    assert 'LD_LIBRARY_PATH' not in os.environ
    for family in ('avcodec', 'avformat', 'avutil', 'swresample', 'mp3lame', 'opus'):
        parent = '/usr/lib' if mode == 'external' else root + '/usr/lib'
        print(f'lib{family}.so.1 => {parent}/lib{family}.so.1 (0x0)')
elif sys.argv[1] == '-n':
    mismatch = mode == 'identity' and '/lib/unsub/' not in sys.argv[-1]
    print('Build ID: ' + ('incorrect' if mismatch else 'pinned'))
elif sys.argv[1] == '--version-info': print('GLIBC_2.36' if mode == 'glibc' else 'GLIBC_2.35')
else: print('RUNPATH [$ORIGIN/../lib]')
'''
            for tool in ('readelf', 'file', 'ldd'):
                (tools / tool).write_text(stub)
                (tools / tool).chmod(0o755)
            env = dict(os.environ, PATH=f'{tools}:{os.environ["PATH"]}', AUDIT_FIXTURE_ROOT=str(package), LD_LIBRARY_PATH='/build/prefix')
            command = ['bash', str(ROOT / 'scripts/audit-linux-package.sh'), str(package)]
            result = subprocess.run(command, env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            for failure in ('external', 'identity', 'glibc'):
                with self.subTest(failure=failure):
                    result = subprocess.run(command, env=dict(env, AUDIT_FIXTURE_MODE=failure), capture_output=True)
                    self.assertNotEqual(result.returncode, 0)

    def test_shared_source_pins(self):
        mac = subprocess.check_output([str(ROOT / 'scripts/build-ffmpeg-macos.sh'), '--print-sources'])
        linux = subprocess.check_output([str(ROOT / 'scripts/build-ffmpeg-linux.sh'), '--print-sources'])
        self.assertEqual(mac, linux)
        self.assertEqual(len(linux.splitlines()), 3)

    def test_linux_rehearsal_cannot_publish_or_skip_tag_platforms(self):
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        guard = "if: github.event_name != 'workflow_dispatch' || !inputs.linux-only"
        for job in ('macos', 'windows'):
            self.assertIn(f'  {job}:\n    {guard}', workflow)
        self.assertIn("  publish:\n    name: Draft release\n    if: github.event_name == 'push' && github.ref_type == 'tag'", workflow)
        self.assertIn('      linux-only:\n        description: Rehearse Linux packages only (never publish a release)\n        type: boolean\n        default: false', workflow)
        self.assertIn('    needs: [macos, windows, linux]', workflow)

    def test_source_directory_selection_ignores_archives(self):
        script = (ROOT / 'scripts/build-ffmpeg-linux.sh').read_text()
        with tempfile.TemporaryDirectory() as directory:
            for family in ('lame', 'opus', 'ffmpeg'):
                name = f'{family}-1.0'
                (Path(directory) / name).mkdir()
                (Path(directory) / f'{name}.tar.gz').touch()
                line = next(line for line in script.splitlines() if line.startswith(f'cd "$WORK"/{family}-'))
                selected = subprocess.check_output(['bash', '-c', f'WORK="$1"; {line}; pwd', '_', directory], text=True).strip()
                self.assertEqual(Path(selected).resolve(), (Path(directory) / name).resolve())

    def test_private_library_layout_and_dependencies(self):
        config = json.loads((ROOT / '.github/tauri/linux-release.conf.json').read_text())['bundle']
        self.assertEqual(set(config['targets']), {'appimage', 'deb'})
        for kind in config['targets']:
            self.assertEqual(config['linux'][kind]['files']['/usr/lib/unsub'], 'ffmpeg-bundle/linux/lib')
            self.assertIn('/usr/share/doc/unsub/licenses', config['linux'][kind]['files'])
        self.assertFalse(any('libav' in dep for dep in config['linux']['deb']['depends']))
        self.assertIn('libc6 (>= 2.35)', config['linux']['deb']['depends'])

    def test_smoke_audio_setup_failure_prevents_application_launch(self):
        script = (ROOT / 'scripts/linux-package-smoke.sh').read_text()
        command = shlex.split(script.split('dbus-run-session -- bash -c ', 1)[1])[0]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            marker = root / 'launched'
            stubs = {
                'pulseaudio': 'test "$FAIL_AT" != pulseaudio',
                'pactl': 'test "$FAIL_AT" != "$1"',
                'timeout': 'touch "$SMOKE_LAUNCH_MARKER"',
            }
            for name, body in stubs.items():
                path = root / name
                path.write_text('#!/bin/sh\n' + body + '\n')
                path.chmod(0o755)
            env = dict(os.environ, PATH=f'{root}:{os.environ["PATH"]}', SMOKE_LAUNCH_MARKER=str(marker))
            args = ['bash', '-c', command, '_', '/fixture/app', '/fixture/profile']
            for failure in ('pulseaudio', 'load-module', 'set-default-sink'):
                with self.subTest(failure=failure):
                    result = subprocess.run(args, env=dict(env, FAIL_AT=failure), capture_output=True)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse(marker.exists())
            self.assertEqual(subprocess.run(args, env=dict(env, FAIL_AT=''), capture_output=True).returncode, 0)
            self.assertTrue(marker.exists())

    def test_bundle_specific_signed_version_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            artifacts = []
            for kind, suffix in [('appimage', 'AppImage'), ('deb', 'deb')]:
                path = Path(directory) / f'Unsub_0.6.0_amd64.{suffix}'
                path.write_bytes(b'fixture package')
                # This tests manifest preflight only; cryptographic verification belongs to updater tests.
                Path(str(path) + '.sig').write_text(base64.b64encode(b'trusted comment: timestamp:0 version:0.6.0\n').decode())
                artifacts.append(f'linux-x86_64-{kind}={path}')
            command = [str(ROOT / 'scripts/updater-manifest.sh'), '--version', '0.6.0', '--tag', 'v0.6.0', '--repo', 'example/unsub', *artifacts]
            manifest = json.loads(subprocess.check_output(command))
            self.assertEqual(set(manifest['platforms']), {'linux-x86_64-appimage', 'linux-x86_64-deb'})
            self.assertTrue(manifest['platforms']['linux-x86_64-deb']['url'].endswith('.deb'))
            Path(artifacts[1].split('=', 1)[1] + '.sig').unlink()
            self.assertNotEqual(subprocess.run(command, capture_output=True).returncode, 0)
            Path(artifacts[1].split('=', 1)[1] + '.sig').write_text(base64.b64encode(b'trusted comment: version:0.5.0\n').decode())
            self.assertNotEqual(subprocess.run(command, capture_output=True).returncode, 0)


if __name__ == '__main__':
    unittest.main()
