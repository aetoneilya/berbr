#!/usr/bin/env python3
"""Install Berbr in a personal copy of the current Herdr App layout.

The existing layout and shipped mods are preserved. Run again to update only
our two mods; pass --activate to switch to the created layout.
"""
import argparse
import datetime
import json
import os
from pathlib import Path
import shutil
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('--activate', action='store_true')
parser.add_argument('--layout', default='berbr')
args = parser.parse_args()
source = Path(__file__).resolve().parent.parent
home = Path(os.environ.get('HERDR_APP_HOME', Path.home()/'.config/herdr-app'))

def app(*args):
    return json.loads(subprocess.check_output(['herdr-app', *args], text=True))

binary = source/'target/release/berbr'
if not binary.is_file():
    raise SystemExit('First run: cargo build --release --locked')
stamp = datetime.datetime.now().strftime('%Y%m%d-%H%M%S')
backup = home/'backups'/f'berbr-{stamp}'
for name in ['berbr', 'tabs-berbr']:
    target = home/'mods'/name
    if target.exists():
        backup.mkdir(parents=True, exist_ok=True)
        shutil.copytree(target, backup/name)
    shutil.copytree(source/'herdr-app'/name, target, dirs_exist_ok=True)
shutil.copy2(binary, home/'mods/berbr/berbr')
for name in ['berbr', 'tabs-berbr']:
    subprocess.run(['herdr-app', 'check', str(home/'mods'/name)], check=True)

layout_file = home/'layouts'/f'{args.layout}.json'
if not layout_file.exists():
    current = app('status')['layout']
    tree = json.loads((home/'layouts'/f'{current}.json').read_text())
    def find(node, target):
        if node.get('id') == target:
            return node
        for child in node.get('children', []):
            result = find(child, target)
            if result is not None:
                return result
        return None
    tabs = find(tree, 'tabs')
    panel = find(tree, 'panel')
    body = find(tree, 'panel-body')
    if not tabs or not panel or not body:
        raise SystemExit('This installer needs the standard tabs / panel / panel-body structure')
    tabs['content']['mod'] = 'tabs-berbr'
    body['children'].append({'id':'berbr', 'type':'block', 'content':{'mod':'berbr'}})
    body['active'] = 'berbr'
    panel['hidden'] = False
    tree['cloned_from'] = {'name':current, 'version':subprocess.check_output(['herdr-app', '--version'], text=True).strip().split()[-1]}
    layout_file.write_text(json.dumps(tree, indent=2)+'\n')
    print(f'Created {layout_file}; original layout: {current}')
if args.activate:
    app('do', 'layout.switch', json.dumps({'name':args.layout}))
print('Installed. Files / Changes / Berbr share the right panel.')
