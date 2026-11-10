import importlib.util
import json
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]

def module(name, file):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS/file)
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded

installer = module('installer', 'install-herdr-app.py')
packager = module('packager', 'package-herdr-app.py')

class Distribution(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.source = self.root/'source'
        mod = self.source/'herdr-app/berbr'
        mod.mkdir(parents=True)
        (mod/'main.js').write_text('export default class Berbr {}')
        (mod/'mod.json').write_text('{"place":"none","title":"Berbr"}')
        (mod/'fixtures').mkdir()
        (mod/'fixtures/private.json').write_text('should not be shipped')
        (self.source/'herdr-app/RECIPE.md').write_text('# Berbr')
        (self.source/'herdr-app/preview.png').write_bytes(b'preview fixture')
        (self.source/'LICENSE').write_text('MIT License')
        (self.source/'scripts').mkdir()
        (self.source/'scripts/install-herdr-app.py').write_text('installer fixture')
        self.binary = self.root/'berbr'
        self.binary.write_text('#!/bin/sh\necho "berbr 0.1.0"\n')
        self.binary.chmod(0o755)
        self.home = self.root/'herdr'
        self.home.mkdir()
        (self.home/'layouts').mkdir()
        (self.home/'layouts/custom.json').write_text('{"custom":true}')
        (self.home/'keys.json').write_text('{"keep":"binding"}')
        bindir = self.root/'bin'
        bindir.mkdir()
        app = bindir/'herdr-app'
        app.write_text('#!/bin/sh\nif [ "$FAIL_CHECK" = 1 ]; then exit 1; fi\nexit 0\n')
        app.chmod(0o755)
        self.addCleanup(patch.stopall)
        patch('platform.system', return_value='Darwin').start()
        patch.dict(os.environ, {'PATH':str(bindir)+os.pathsep+os.environ.get('PATH','')}).start()

    def test_install_update_and_failed_check_preserve_personal_files(self):
        installer.install(self.source, self.home, self.binary)
        target = self.home/'mods/berbr'
        self.assertTrue((target/'berbr').is_file())
        self.assertFalse((target/'fixtures').exists())
        self.assertEqual(json.loads((target/'mod.json').read_text())['place'],'right')
        (target/'old-marker').write_text('previous version')
        with patch.dict(os.environ, {'FAIL_CHECK':'1'}):
            with self.assertRaises(Exception):
                installer.install(self.source, self.home, self.binary)
        self.assertEqual((target/'old-marker').read_text(),'previous version')
        installer.install(self.source, self.home, self.binary)
        self.assertFalse((target/'old-marker').exists())
        self.assertEqual(len(list((self.home/'backups').glob('berbr-*'))),1)
        self.assertEqual((self.home/'layouts/custom.json').read_text(),'{"custom":true}')
        self.assertEqual((self.home/'keys.json').read_text(),'{"keep":"binding"}')

    def test_release_and_gallery_are_installable_shapes_without_test_files(self):
        with patch.object(packager, 'ROOT', self.source):
            output = self.root/'dist'
            archive = packager.package(self.binary,'aarch64-apple-darwin','v0.2.0-preview.1',output)
            with tarfile.open(archive) as tar:
                names = tar.getnames()
                self.assertTrue(any(p.endswith('/mods/berbr/berbr') for p in names))
                self.assertTrue(any(p.endswith('/install.py') for p in names))
                self.assertFalse(any('/fixtures' in p for p in names))
            self.assertTrue(archive.with_suffix('.gz.sha256').is_file())
            gallery = packager.package(None,'aarch64-apple-darwin','v0.2.0-preview.1',output,True)
            self.assertTrue((gallery/'mods/berbr/main.js').is_file())
            self.assertFalse((gallery/'mods/berbr/berbr').exists())
            self.assertFalse((gallery/'layouts').exists())
            self.assertTrue((gallery/'RECIPE.md').is_file())
            self.assertEqual((gallery/'LICENSE').read_text(),'MIT License')
            with self.assertRaises(ValueError):
                packager.package(None,'aarch64-apple-darwin','v0.2.0-preview.1',output,True)
