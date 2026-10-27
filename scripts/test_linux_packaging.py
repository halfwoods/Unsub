"""Host-independent regressions for Linux packaging and updater routing."""
import base64
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


class LinuxPackagingTests(unittest.TestCase):
    def test_shared_source_pins(self):
        mac = subprocess.check_output([str(ROOT / 'scripts/build-ffmpeg-macos.sh'), '--print-sources'])
        linux = subprocess.check_output([str(ROOT / 'scripts/build-ffmpeg-linux.sh'), '--print-sources'])
        self.assertEqual(mac, linux)
        self.assertEqual(len(linux.splitlines()), 3)

    def test_private_library_layout_and_dependencies(self):
        config = json.loads((ROOT / '.github/tauri/linux-release.conf.json').read_text())['bundle']
        self.assertEqual(set(config['targets']), {'appimage', 'deb'})
        for kind in config['targets']:
            self.assertEqual(config['linux'][kind]['files']['/usr/lib/unsub'], 'ffmpeg-bundle/linux/lib')
            self.assertIn('/usr/share/doc/unsub/licenses', config['linux'][kind]['files'])
        self.assertFalse(any('libav' in dep for dep in config['linux']['deb']['depends']))

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
