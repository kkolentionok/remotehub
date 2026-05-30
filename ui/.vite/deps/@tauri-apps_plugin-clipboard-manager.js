import {
  Image,
  transformImage
} from "./chunk-F5F3O7SA.js";
import {
  invoke
} from "./chunk-H3D4IWB6.js";
import "./chunk-G3PMV62Z.js";

// node_modules/.pnpm/@tauri-apps+plugin-clipboard-manager@2.3.2/node_modules/@tauri-apps/plugin-clipboard-manager/dist-js/index.js
async function writeText(text, opts) {
  await invoke("plugin:clipboard-manager|write_text", {
    label: opts == null ? void 0 : opts.label,
    text
  });
}
async function readText() {
  return await invoke("plugin:clipboard-manager|read_text");
}
async function writeImage(image) {
  await invoke("plugin:clipboard-manager|write_image", {
    image: transformImage(image)
  });
}
async function readImage() {
  return await invoke("plugin:clipboard-manager|read_image").then((rid) => new Image(rid));
}
async function writeHtml(html, altText) {
  await invoke("plugin:clipboard-manager|write_html", {
    html,
    altText
  });
}
async function clear() {
  await invoke("plugin:clipboard-manager|clear");
}
export {
  clear,
  readImage,
  readText,
  writeHtml,
  writeImage,
  writeText
};
//# sourceMappingURL=@tauri-apps_plugin-clipboard-manager.js.map
