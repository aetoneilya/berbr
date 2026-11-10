#!/usr/bin/env python3
"""Capture the real native panel with synthetic data in an isolated Herdr home."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent


def snapshot():
    def row(kind, id, label, **extra):
        return dict(kind=kind, id=id, label=label, children=[], **extra)
    def task(id, title, state, children):
        return dict(kind='task', id=id, task=id, title=title, label=title, state=state, done=False,
                    note=f'/home/demo/notes/{id}.md', body='Synthetic gallery example.', children=children)
    t1 = row('tree','cache-tree','cache-isolation',tree='cache-isolation',branch='fix/cache-isolation',cwd='/home/demo/work/cache-isolation')
    t1['children']=[row('agent','worker','Codex',terminal='worker',state='working')]
    t2 = row('tree','upload-tree','upload-retry',tree='upload-retry',branch='fix/upload-retry',cwd='/home/demo/work/upload-retry')
    t2['children']=[row('agent','waiting','Claude Code',terminal='waiting',state='blocked')]
    mr=row('mr','mr-42','#42  Keep settings across restarts',state='opened',provider='github',repository='example/demo',url='https://github.com/example/demo/pull/42',
           branch='fix/settings',ci='success',approvals=1,unresolved=0,draft=False,conflict=False,merge_status='mergeable',ready=True)
    retry=dict(mr,id='mr-43',label='#43  Retry interrupted uploads',url='https://github.com/example/demo/pull/43',
        branch='fix/upload-retry',ci='failed',approvals=0,merge_status='ci_must_pass',ready=False)
    shortcuts=dict(mr,id='mr-44',label='#44  Add keyboard shortcuts',url='https://github.com/example/demo/pull/44',
        branch='feat/shortcuts',ci='pending',draft=True,approvals=0,merge_status='draft_status',ready=False)
    return {'version':3,'project':'Demo workspace','projects':[{'name':'Demo workspace','agent':'codex'}],
        'merge_requests':[mr,retry,shortcuts],
        'rows':[task('demo-1','Isolate the cache per project','active',[t1]),task('demo-2','Retry interrupted uploads','active',[t2]),
                task('demo-3','Keep settings across restarts','review',[mr]),task('demo-4','Add keyboard shortcuts','idea',[])],
        'warnings':[],'store':'berbr','settings':{'agent':'codex','generation_agent':'codex','generation_model':''}}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,default=ROOT/'herdr-app/preview.png')
    parser.add_argument('--setup',action='store_true')
    parser.add_argument('--screen',choices=['tasks','mrs','new','details'],default='tasks')
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='berbr-shot-', dir='/tmp') as tmp:
        home=Path(tmp)
        mod=home/'mods/berbr'
        shutil.copytree(ROOT/'herdr-app/berbr',mod)
        data={'setup_required':True,'rows':[],'projects':[],'warnings':[]} if args.setup else snapshot()
        fake_agents=[{'address':'worker','pane':'worker','status':'working','app':'codex','title':'Codex · Isolating the cache','seq':1},
                     {'address':'waiting','pane':'waiting','status':'blocked','app':'claude','title':'Claude Code · Waiting for your answer','seq':1}]
        shim='''export * from "herdr";
export const agents = () => AGENTS;
export const spaces = () => [];
export const panes = () => [];
export const focused = () => null;
export const exec = async (_program, args) => args.includes("app-snapshot")
  ? {code:0,stdout:JSON.stringify(SNAPSHOT),stderr:""}
  : {code:1,stdout:"",stderr:"Synthetic preview: external actions are disabled"};
export const hc = async () => ({code:0,stdout:'{}',stderr:""});
'''.replace('AGENTS',json.dumps(fake_agents)).replace('SNAPSHOT',json.dumps(data))
        (mod/'preview-host.js').write_text(shim)
        for name in ['main.js','forms.js','log.js']:
            p=mod/name;p.write_text(p.read_text().replace('from "herdr"','from "./preview-host.js"'))
        if args.screen == 'details':
            p=mod/'main.js'
            p.write_text(p.read_text().replace('this.expanded = null;', 'this.expanded = "demo-3";'))
        if args.screen == 'mrs':
            p=mod/'main.js'
            p.write_text(p.read_text().replace('return localStorage.getItem(`berbr.view.${project}`) === "mrs" ? "mrs" : "tasks";', 'return "mrs";'))
        panel_props = {'form':'new','project':'Demo workspace','projects':['Demo workspace'],
            'prefill':{'description':'Add retries for interrupted uploads and keep progress visible.'}} if args.screen == 'new' else {}
        (home/'layouts').mkdir()
        (home/'layouts/preview.json').write_text(json.dumps({'id':'root','type':'column','titlebar':'none','background':'canvas',
            'children':[{'id':'berbr','type':'block','content':{'mod':'berbr','props':panel_props}}]}))
        (home/'window.json').write_text(json.dumps({'state':'windowed','bounds':[180,100,580,560]}))
        (home/'shared.json').write_text(json.dumps({'frame.welcomed':'finished','layout':'preview'}))
        version=subprocess.check_output(['herdr-app','--version'],text=True).strip().split()[-1]
        (home/'update-notes.json').write_text(json.dumps({'last_run':version,'presented':[version]}))
        (home/'settings.json').write_text(json.dumps({'appearance':'dark','sounds':{'enabled':False},'zoom':1}))
        env=dict(os.environ,HERDR_APP_HOME=str(home),HC_SOCKET=str(home/'core.sock'),HC_SESSION='berbr-preview',
            XDG_STATE_HOME=str(home/'state'),XDG_CONFIG_HOME=str(home/'config'),XDG_DATA_HOME=str(home/'data'),HERDR_APP_NO_SOUND='1')
        for name in ['HC_TERMINAL_ID','HC_TERMINAL_SOCKET','HERDR_PLUGIN_CONTEXT_JSON','HERDR_PLUGIN_ROOT','HERDR_ENV','HERDR_WORKSPACE_ID','HERDR_TAB_ID','HERDR_PANE_ID']:
            env.pop(name,None)
        # Only the temporary home and explicit socket created here are controlled.
        with (home/'app.log').open('w') as log:
            app=subprocess.Popen(['herdr-app'],env=env,stdout=log,stderr=log)
            try:
                deadline=time.monotonic()+25
                while time.monotonic()<deadline:
                    result=subprocess.run(['herdr-app','status'],env=env,capture_output=True,text=True)
                    if result.returncode==0: break
                    if app.poll() is not None: raise RuntimeError((home/'app.log').read_text()[-5000:])
                    time.sleep(.25)
                else: raise RuntimeError('Preview window did not become ready: '+result.stderr+'\n'+(home/'app.log').read_text()[-5000:])
                subprocess.run(['herdr-app','do','layout.switch',json.dumps({'name':'preview'})],env=env,check=True,capture_output=True)
                time.sleep(1)
                status=json.loads(subprocess.check_output(['herdr-app','status'],env=env,text=True))
                if status.get('errors') or status.get('overlays'): raise RuntimeError(json.dumps(status))
                print(json.dumps(status))
                args.output.parent.mkdir(parents=True,exist_ok=True)
                subprocess.run(['herdr-app','shot',str(args.output.resolve())],env=env,check=True)
            finally:
                app.terminate()
                try: app.wait(timeout=5)
                except subprocess.TimeoutExpired: app.kill();app.wait()
                if (home/'core.sock').exists():
                    subprocess.run(['hc','server','stop'],env=env,capture_output=True,timeout=5)


if __name__=='__main__':
    main()
