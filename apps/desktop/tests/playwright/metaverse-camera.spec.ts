import { expect, test } from '@playwright/test';
import { DEVELOPER_MODE_STORAGE_KEY } from '../../src/lib/developerMode';

type CameraProbe = { cameraMoves: { position: number[]; animation: string }[]; cameraModelViewMatrix?: number[] };

test('camera capture, real wheel and keyboard UI round trips preserve the Column and draft', async ({ page }) => {
  // A cold 3D startup plus capture, wheel, keyboard, chat and camera-button round trips
  // exceeds the default 30s on the shared CI runner; each assertion keeps its deadline.
  test.slow();
  // Cover the primitive fallback and avoid asynchronous VRM bounds changes in the matrix probe.
  await page.route('**/*.vrm', route => route.abort());
  await page.addInitScript(key => localStorage.setItem(key, 'true'), DEVELOPER_MODE_STORAGE_KEY);
  await page.addInitScript(() => {
    // Observe the real rendered camera, without a production test/debug API.
    const names = new WeakMap<WebGLUniformLocation, string>();
    const prototype = WebGL2RenderingContext.prototype;
    const getLocation = prototype.getUniformLocation;
    const matrix = prototype.uniformMatrix4fv;
    prototype.getUniformLocation = function (program, name) {
      const location = getLocation.call(this, program, name);
      if (location) names.set(location, name);
      return location;
    };
    prototype.uniformMatrix4fv = function (location, transpose, data, ...rest) {
      if (location && names.get(location) === 'modelViewMatrix') {
        (window as unknown as CameraProbe).cameraModelViewMatrix = Array.from(data);
      }
      return matrix.call(this, location, transpose, data, ...rest);
    };
  });
  await page.setViewportSize({ width: 1280, height: 870 });
  await page.goto('/#/timeline?topic=kukuri%3Atopic%3Ageneral');
  await page.getByTestId('control-center-trigger').click();
  await page.getByRole('button', { name: 'Add Metaverse Column' }).click();
  const column = page.getByRole('region', { name: /^Metaverse Column/ });
  const create = column.getByRole('button', { name: 'Create metaverse room' }).first();
  if (await create.getAttribute('aria-expanded') === 'false') await create.click();
  await column.getByPlaceholder('Atrium').fill('Camera regression');
  await column.getByRole('button', { name: 'Create metaverse room' }).last().click();
  await page.evaluate(() => {
    const api = window.__KUKURI_DESKTOP__!;
    const submit = api.submitDomeSessionInput.bind(api);
    const probe = window as unknown as CameraProbe;
    probe.cameraMoves = [];
    api.submitDomeSessionInput = (...args) => {
      if (args[3].type === 'move') probe.cameraMoves.push({ position: [...args[3].position], animation: String(args[3].animation) });
      return submit(...args);
    };
  });
  const clearMoves = () => page.evaluate(() => { (window as unknown as CameraProbe).cameraMoves = []; });
  const moves = () => page.evaluate(() => (window as unknown as CameraProbe).cameraMoves);
  await column.getByRole('button', { name: 'Start hosting and enter' }).click();
  const stage = column.locator('[data-column-gesture-owner="metaverse"]');
  await expect(stage).toBeVisible();
  // A loaded renderer can publish its first transform much later than DOM visibility.
  await expect.poll(async () => (await moves()).at(-1)?.animation).toBe('idle');
  await clearMoves();
  await expect(stage).toHaveAttribute('data-input-mode', 'idle');
  await stage.locator('canvas').click({ position: { x: 550, y: 260 } });
  await expect(stage).toHaveAttribute('data-input-mode', 'locked');
  await expect.poll(() => page.evaluate(() => document.pointerLockElement?.tagName)).toBe('CANVAS');
  const scrolls = () => column.evaluate(el => [el.scrollTop, el.querySelector('.shell-column-body')?.scrollTop, document.scrollingElement?.scrollTop]);
  const before = await scrolls();
  const viewMatrix = () => page.evaluate(() => (window as unknown as CameraProbe).cameraModelViewMatrix);
  await expect.poll(viewMatrix).toHaveLength(16);
  const initialView = await viewMatrix();
  await page.waitForTimeout(100);
  expect(await viewMatrix()).toEqual(initialView);
  const box = (await stage.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2 + 100, box.y + box.height / 2);
  await expect.poll(viewMatrix).not.toEqual(initialView);
  const rotatedView = await viewMatrix();
  await page.mouse.wheel(0, 180);
  await expect.poll(viewMatrix).not.toEqual(rotatedView);
  await page.waitForTimeout(100);
  expect(await scrolls()).toEqual(before);
  expect(await moves()).toEqual([]);
  await page.keyboard.down('w');
  await expect.poll(async () => (await moves()).at(-1)?.animation).toBe('walk');
  await page.keyboard.press('Tab');
  await expect(stage).toHaveAttribute('data-input-mode', 'idle');
  await expect(stage.getByRole('button', { name: 'Dome settings' })).toBeFocused();
  expect(await page.evaluate(() => document.pointerLockElement)).toBeNull();
  // Opening UI clears held keys even if keyup arrives outside the canvas.
  await expect.poll(async () => (await moves()).at(-1)?.animation).toBe('idle');
  await clearMoves();
  await page.keyboard.up('w');
  await page.waitForTimeout(200);
  expect(await moves()).toEqual([]);
  await page.keyboard.press('Escape');
  await expect(stage).toBeFocused();
  await page.keyboard.press('Enter');
  const chat = stage.getByLabel('Room chat message');
  await expect(chat).toBeFocused();
  await chat.fill('camera draft');
  await page.keyboard.press('Escape');
  await expect(chat).toHaveCount(0);
  await page.keyboard.press('Enter');
  await expect(chat).toHaveValue('camera draft');
  await page.keyboard.press('Escape');
  await stage.getByText('Adjust view', { exact: true }).click();
  await stage.getByRole('button', { name: 'Reset camera' }).click();
  await stage.getByRole('button', { name: 'Resume avatar controls' }).click();
  await expect(stage).toHaveAttribute('data-input-mode', 'locked');
  await page.keyboard.press('Escape');
  await expect(stage).toHaveAttribute('data-input-mode', 'idle');
  await expect(column).toHaveAttribute('aria-current', 'true');
});
