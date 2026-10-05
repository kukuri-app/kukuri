// Tauri の build で `@kukuri/web-runtime` の代わりに解決する（vite.config.ts）。Web の build でだけ読む。
const unavailable = (): never => {
  throw new Error('the web runtime is only available in the web build');
};
export default unavailable;
export const start = unavailable;
export const invoke = unavailable;
export const listen = unavailable;
