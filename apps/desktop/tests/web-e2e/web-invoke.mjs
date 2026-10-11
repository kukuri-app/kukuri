export function webInvoke(browser, command, args = {}) {
  return browser.execute(async (command, args) => {
    const module = performance.getEntriesByType('resource').find(entry =>
      entry.name.includes('/kukuri_web_runtime-') && entry.name.endsWith('.js'));
    if (!module) throw new Error('web-runtime module was not loaded');
    try { return await (await import(module.name)).invoke(command, args); }
    catch (error) { throw new Error(`${command}: ${JSON.stringify(error)}`, { cause: error }); }
  }, command, args);
}
