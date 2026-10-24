// The Rust root tested from TypeScript: `nsh test` runs this file in a
// headless runner it builds with the crate, and `open(".")` starts the root
// (`nsh.json` "root"). The app auto-starts the embedded Fiber node, so this
// only asserts the shell mounts.
import { expect, open, test } from "nsh/test";

test("wallet shell mounts", async () => {
  const app = await open(".");
  expect(app.texts()).toContain("Fiber Wallet");
  expect(app.texts()).toContain("首页");
  expect(app.texts()).toContain("交易");
  expect(app.texts()).toContain("发现");
  expect(app.texts()).toContain("设置");
});
