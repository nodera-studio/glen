import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import { removeRegistryIfUnchanged } from "../index.ts";
test("stale cleanup verifies the renamed registry and restores a fresh publisher", () => {
    const home = fs.mkdtempSync("/tmp/glen-reg-");
    try {
        const file = `${home}/session.json`;
        const stale = JSON.stringify({ pid: 12, started_at: "old" });
        const fresh = JSON.stringify({ pid: 34, started_at: "new" });
        fs.writeFileSync(file, fresh);
        assert.equal(removeRegistryIfUnchanged(file, stale), false);
        assert.equal(fs.readFileSync(file, "utf8"), fresh);
        assert.deepEqual(fs.readdirSync(home), ["session.json"]);
        assert.equal(removeRegistryIfUnchanged(file, fresh), true);
        assert.equal(fs.existsSync(file), false);
    } finally { fs.rmSync(home, { recursive: true, force: true }); }
});
