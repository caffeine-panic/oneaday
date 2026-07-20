import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = readFileSync(
  new URL("../src/registryError.ts", import.meta.url),
  "utf8",
);
const output = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ES2022,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const errors = await import(
  `data:text/javascript;base64,${Buffer.from(output).toString("base64")}`
);

test("registry errors are classified exclusively by their structured code", () => {
  const auditIncomplete = {
    code: "auditIncomplete",
    message: "localized or rewritten message",
    retryable: false,
  };

  assert.equal(errors.isRegistryError(auditIncomplete, "auditIncomplete"), true);
  assert.equal(errors.isRegistryError(auditIncomplete, "outcomeUnknown"), false);
  assert.equal(
    errors.isRegistryError(
      { code: "network", message: "mutation succeeded", retryable: true },
      "auditIncomplete",
    ),
    false,
  );
  assert.equal(errors.isRegistryError("auditIncomplete", "auditIncomplete"), false);
  assert.equal(errors.isRegistryError({ code: "auditIncomplete" }, "auditIncomplete"), false);
  assert.equal(
    errors.isRegistryError(
      { code: "notARegistryCode", message: "bad", retryable: false },
      "notARegistryCode",
    ),
    false,
  );
});

test("registry error messages remain safe for unknown rejection values", () => {
  assert.equal(errors.registryErrorMessage({ message: "remote failed" }), "remote failed");
  assert.equal(errors.registryErrorMessage("plain failure"), "plain failure");
  assert.equal(errors.registryErrorMessage(undefined), "undefined");
});

test("mutation failures select recovery behavior from structured codes", () => {
  const failure = (code) => ({ code, message: "translated", retryable: false });
  assert.equal(errors.mutationFailureRecovery(failure("outcomeUnknown")), "unknownOutcome");
  assert.equal(errors.mutationFailureRecovery(failure("auditIncomplete")), "unknownOutcome");
  assert.equal(errors.mutationFailureRecovery(failure("conflict")), "conflict");
  assert.equal(errors.mutationFailureRecovery(failure("network")), "report");
});
