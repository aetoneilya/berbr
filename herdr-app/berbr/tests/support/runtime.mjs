import { readFileSync } from "node:fs";

// A recording builder checks method names against the shipped API, not layout or rendering.
const api = readFileSync(new URL("../../gpui-kit.d.ts", import.meta.url), "utf8");
const builders = new Set([...api.matchAll(/^\s*(\w+)(?:<[^\n]+>)?\(/gm)].map(match => match[1]));
export function div() {
  const node = {nodes:[], attributes:{}};
  const proxy = new Proxy(node, {get(target, key) {
    if (key in target) return target[key];
    if (key === "then") return undefined;
    if (!builders.has(key)) throw new Error(`Unknown GPUI builder: ${key}`);
    return (...args) => {
      if (key === "child") node.nodes.push(args[0]);
      else if (key === "children") node.nodes.push(...args[0]);
      else if (key === "when") { if (args[0]) args[1](proxy); }
      else node.attributes[key] = args;
      if (["on_mouse_down", "on_mouse_up"].includes(key)) {
        node.handlers ??= {};
        node.handlers[`${key}:${args[0]}`] = args[1];
      }
      return proxy;
    };
  }});
  return proxy;
}
export const h_flex = div, v_flex = div;
export const svg = path => Object.assign(div().child(path), {asset:{kind:"svg", path}});
export const image = path => Object.assign(div().child(path), {asset:{kind:"image", path}});
export class View {}
export const Input = {new:input => Object.assign(div(), {input})};
export const Switch = {new:() => div()};
export const InputState = {new:({value, placeholder}) => {
  const listeners = new Map();
  return {placeholder, on:(event, fn) => listeners.set(event, fn), focus() {}, value:() => value,
    set_value(next) {value = next;}, emit:(event, cx) => listeners.get(event)?.({}, cx)};
}};
export const TextView = {html:(_id, text) => div().child(text), markdown:(_id, text) => div().child(text)};
export const state = {props:{}, spaces:[], agents:[], shared:new Map(), calls:[], overlays:[], notices:[], execs:[]};
export const appHome = () => "/test";
export const focused = () => state.focused ?? null;
export const props = () => state.props;
export const spaces = () => state.spaces;
export const agents = () => state.agents;
export const panes = () => state.panes ?? [];
export const theme = () => new Proxy({}, {get:(_target, name) => name});
export const shared = key => state.shared.get(key);
export const setShared = (key, value) => state.shared.set(key, value);
export const run = (...args) => state.calls.push(args);
export const openOverlay = spec => state.overlays.push(spec);
export const closeOverlay = id => state.calls.push(["closeOverlay", id]);
export const notify = message => state.notices.push(message);
export const nextCommand = () => new Promise(() => {});
export const readTextFile = async path => {
  state.reads.push(path);
  if (state.readError) throw state.readError;
  return state.files ? state.files.get(path) ?? null : state.readResult;
};
export const watchFolder = path => {
  state.watches.push(path);
  return new Promise((resolve, reject) => state.watchers.push({resolve, reject}));
};
export const exec = async (...args) => {
  state.execs.push(args);
  if (state.execHandler) return state.execHandler(...args);
  return state.execResult ?? {code:0, stdout:"", stderr:""};
};
export const hc = async args => {
  state.hcCalls.push(args);
  if (state.hcError) throw state.hcError;
  if (state.hcHandler) return state.hcHandler(args);
  return state.hcResult ?? {code:0, stdout:"", stderr:""};
};
export const settings = () => ({font_family:"Test Mono"});
export const Editor = {new:editor => Object.assign(div(), {editor})};
export const EditorState = {new:options => {
  let value = options.value;
  const listeners = new Map();
  return {options, value:() => value, set_value(next) {value = next;},
    edit(next, cx) { value = next; listeners.get("change")?.({}, cx); },
    on(event, callback) {listeners.set(event, callback);}, release() {}};
}};
export const writeTextFile = async (path, value, expected) => {
  state.writes.push({path, value, expected});
  if (state.writeError) throw state.writeError;
  if (state.writeResult) return state.writeResult;
  if (state.files) {
    if ((state.files.get(path) ?? null) !== expected) throw new Error("File changed on disk");
    state.files.set(path, value);
  }
  return null;
};

// fs/promises.exists is a host API; keep note tests on synthetic in-memory files.
export const exists = async path => {
  (state.existsCalls ??= []).push(path);
  if (state.existsError) throw state.existsError;
  return state.files ? state.files.has(path) : !state.readError && state.readResult !== null;
};
// The mod log's files, in memory.
export const readFile = async path => {
  const text = (state.fsFiles ??= new Map()).get(path);
  if (text === undefined) throw new Error(`ENOENT: ${path}`);
  return text;
};
export const writeFile = async (path, text) => { (state.fsFiles ??= new Map()).set(path, String(text)); };
export const mkdir = async () => {};
export const layout = () => state.layout ?? {children:[]};
export const window = () => state.window ?? null;
