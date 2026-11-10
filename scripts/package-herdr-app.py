#!/usr/bin/env python3
"""Build a macOS release archive and a source-only folder for the Herdr gallery."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parent.parent
TARGETS = ['aarch64-apple-darwin', 'x86_64-apple-darwin']


def package(binary, target, version, output, gallery=False):
    if target not in TARGETS or not version or any(c not in '0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-' for c in version):
        raise ValueError('Use a macOS target and a version containing letters, digits, dots or hyphens.')
    preview = ROOT/'herdr-app/preview.png'
    if not preview.is_file():
        raise ValueError('First capture a synthetic Herdr screenshot at herdr-app/preview.png.')
    output.mkdir(parents=True, exist_ok=True)
    name = 'berbr-gallery' if gallery else f'berbr-{version}-{target}'
    destination = output/name
    if destination.exists():
        raise ValueError(f'{destination} exists; choose another output folder or remove that build first.')
    with tempfile.TemporaryDirectory(prefix='.berbr-package-', dir=output) as tmp:
        stage = Path(tmp)/name
        shutil.copytree(ROOT/'herdr-app/berbr', stage/'mods/berbr', ignore=shutil.ignore_patterns('tests', 'fixtures', '*.d.ts', 'jsconfig.json', 'berbr'))
        manifest = json.loads((stage/'mods/berbr/mod.json').read_text())
        manifest.update(place='right', size=420)
        (stage/'mods/berbr/mod.json').write_text(json.dumps(manifest, indent=2)+'\n')
        shutil.copy2(ROOT/'herdr-app/RECIPE.md', stage/'RECIPE.md')
        shutil.copy2(preview, stage/'preview.png')
        shutil.copy2(ROOT/'LICENSE', stage/'LICENSE')
        shutil.copy2(ROOT/'scripts/install-herdr-app.py', stage/'install.py')
        if (ROOT/'skills/berbr/SKILL.md').is_file():
            shutil.copytree(ROOT/'skills/berbr', stage/'skills/berbr')
        if not gallery:
            if not binary or not binary.is_file():
                raise ValueError('Pass --binary <release backend>.')
            shutil.copy2(binary, stage/'mods/berbr/berbr')
            (stage/'mods/berbr/berbr').chmod(0o755)
        metadata = {'version':version, 'target':None if gallery else target, 'source_only':gallery}
        (stage/'release.json').write_text(json.dumps(metadata, indent=2)+'\n')
        stage.rename(destination)
    if gallery:
        return destination
    archive = output/f'{name}.tar.gz'
    with tarfile.open(archive, 'w:gz') as tar:
        tar.add(destination, arcname=name)
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix(archive.suffix+'.sha256').write_text(f'{checksum}  {archive.name}\n')
    return archive


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path)
    parser.add_argument('--target', choices=TARGETS, default=TARGETS[0])
    parser.add_argument('--version', required=True)
    parser.add_argument('--output', type=Path, default=ROOT/'dist')
    parser.add_argument('--gallery', action='store_true', help='Source-only gallery folder; no binary')
    args = parser.parse_args()
    try:
        print(package(args.binary, args.target, args.version, args.output, args.gallery))
    except (OSError, ValueError) as error:
        raise SystemExit(str(error))


if __name__ == '__main__':
    main()
