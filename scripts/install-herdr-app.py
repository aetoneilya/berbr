#!/usr/bin/env python3
"""Install a matched Berbr mod/backend; preserve layouts, keys and config."""
import argparse
import datetime
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile


def source_root():
    here = Path(__file__).resolve().parent
    return next((p for p in (here, here.parent) if (p/'mods/berbr/main.js').is_file() or (p/'herdr-app/berbr/main.js').is_file()), here.parent)


def install(source, home, binary, activate=False):
    mod = source/'mods/berbr'
    if not mod.is_dir():
        mod = source/'herdr-app/berbr'
    if not (mod/'main.js').is_file() or not binary.is_file():
        raise ValueError('Use an unpacked macOS release, or pass --binary <built berbr> from a checkout.')
    if platform.system() != 'Darwin':
        raise ValueError('This preview supports macOS. Linux support is not yet verified.')
    subprocess.run([str(binary), '--version'], check=True, capture_output=True)
    home.mkdir(parents=True, exist_ok=True)
    (home/'mods').mkdir(exist_ok=True)
    target = home/'mods/berbr'
    # Validate in a separate home before replacing the installed mod. No live app
    # commands, config writes, layout changes or access to personal task notes.
    with tempfile.TemporaryDirectory(prefix='.berbr-install-', dir=home) as tmp:
        stage_home = Path(tmp)
        staged = stage_home/'mods/berbr'
        shutil.copytree(mod, staged, ignore=shutil.ignore_patterns('tests', 'fixtures', '*.d.ts', 'jsconfig.json'))
        shutil.copy2(binary, staged/'berbr')
        (staged/'berbr').chmod(0o755)
        manifest = json.loads((staged/'mod.json').read_text())
        manifest.update(place='right', size=420)
        (staged/'mod.json').write_text(json.dumps(manifest, indent=2)+'\n')
        env = dict(os.environ, HERDR_APP_HOME=str(stage_home), BERBR_CONFIG=str(stage_home/'test-config.toml'))
        subprocess.run(['herdr-app', 'check', str(staged)], check=True, env=env)
        stamp = datetime.datetime.now().strftime('%Y%m%d-%H%M%S-%f')
        backup = home/'backups'/f'berbr-{stamp}'
        if target.is_symlink():
            raise ValueError('The installed berbr mod is a symlink. Resolve it before updating.')
        if target.exists():
            try:
                existing = json.loads((target/'mod.json').read_text())
            except (OSError, ValueError):
                existing = {}
            if existing.get('title') != 'Berbr':
                raise ValueError('The berbr mod name is occupied by another mod. Rename that mod or adapt the installation name first.')
            backup.parent.mkdir(parents=True, exist_ok=True)
            target.rename(backup)
        try:
            staged.rename(target)
        except OSError:
            if backup.exists():
                backup.rename(target)
            raise
    if activate:
        subprocess.run(['herdr-app', 'do', 'mod.open', json.dumps({'name':'berbr'})], check=True)
    print(f'Installed {target}. Layouts, keybindings and project config are preserved.')
    print(f'CLI: {target / "berbr"} (add this directory to PATH if you use Berbr from a terminal).')
    print('Open Berbr from the command palette, or run again with --activate.')
    if backup.exists():
        print(f'Previous mod/backend: {backup}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--activate', action='store_true', help='Open Berbr after installation')
    parser.add_argument('--binary', type=Path, help='A built backend; release archives include it')
    args = parser.parse_args()
    source = source_root()
    binary = args.binary or (source/'mods/berbr/berbr' if (source/'mods/berbr/berbr').is_file() else source/'target/release/berbr')
    home = Path(os.environ.get('HERDR_APP_HOME', Path.home()/'.config/herdr-app')).expanduser().resolve()
    try:
        install(source, home, binary.expanduser().resolve(), args.activate)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(f'Installation failed: {error}')


if __name__ == '__main__':
    main()
