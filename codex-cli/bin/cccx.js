#!/usr/bin/env node
// Unified entry point for the cccx CLI.

import { spawn } from "node:child_process";
import { existsSync, realpathSync } from "fs";
import { createRequire } from "node:module";
import path from "path";
import { fileURLToPath } from "url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const require = createRequire(import.meta.url);
const codexPackageRoot = realpathSync(path.join(__dirname, ".."));

const PLATFORM_PACKAGE_BY_TARGET = {
  "x86_64-unknown-linux-musl": "@openai/codex-linux-x64",
  "aarch64-unknown-linux-musl": "@openai/codex-linux-arm64",
  "x86_64-apple-darwin": "@openai/codex-darwin-x64",
  "aarch64-apple-darwin": "@openai/codex-darwin-arm64",
  "x86_64-pc-windows-msvc": "@openai/codex-win32-x64",
  "aarch64-pc-windows-msvc": "@openai/codex-win32-arm64",
};

const { platform, arch } = process;

let targetTriple = null;
switch (platform) {
  case "linux":
  case "android":
    switch (arch) {
      case "x64":
        targetTriple = "x86_64-unknown-linux-musl";
        break;
      case "arm64":
        targetTriple = "aarch64-unknown-linux-musl";
        break;
      default:
        break;
    }
    break;
  case "darwin":
    switch (arch) {
      case "x64":
        targetTriple = "x86_64-apple-darwin";
        break;
      case "arm64":
        targetTriple = "aarch64-apple-darwin";
        break;
      default:
        break;
    }
    break;
  case "win32":
    switch (arch) {
      case "x64":
        targetTriple = "x86_64-pc-windows-msvc";
        break;
      case "arm64":
        targetTriple = "aarch64-pc-windows-msvc";
        break;
      default:
        break;
    }
    break;
  default:
    break;
}

if (!targetTriple) {
  throw new Error(`Unsupported platform: ${platform} (${arch})`);
}

const platformPackage = PLATFORM_PACKAGE_BY_TARGET[targetTriple];
if (!platformPackage) {
  throw new Error(`Unsupported target triple: ${targetTriple}`);
}

function findExecutable() {
  let vendorRoot;
  try {
    const packageJsonPath = require.resolve(`${platformPackage}/package.json`);
    vendorRoot = path.join(path.dirname(packageJsonPath), "vendor");
  } catch {
    vendorRoot = path.join(__dirname, "..", "vendor");
  }

  for (const executableName of [
    process.platform === "win32" ? "cccx.exe" : "cccx",
    process.platform === "win32" ? "codex.exe" : "codex",
  ]) {
    const candidate = path.join(vendorRoot, targetTriple, "bin", executableName);
    if (existsSync(candidate)) {
      return candidate;
    }
  }

  const packageManager = detectPackageManager();
  const updateCommand =
    packageManager === "bun"
      ? "bun install -g cccx@latest"
      : packageManager === "pnpm"
        ? "pnpm add -g cccx@latest"
        : "npm install -g cccx@latest";
  throw new Error(
    `Missing optional dependency ${platformPackage}. Reinstall cccx: ${updateCommand}`,
  );
}

const binaryPath = findExecutable();

function isPnpmOwnedInstall(nodeModulesDir) {
  if (!existsSync(path.join(nodeModulesDir, ".modules.yaml"))) {
    return false;
  }

  for (const packageRoot of [
    path.join(nodeModulesDir, "cccx"),
    path.join(nodeModulesDir, "@openai", "codex"),
  ]) {
    try {
      if (realpathSync(packageRoot) === codexPackageRoot) {
        return true;
      }
    } catch {
      // ignore compatibility path lookup failures
    }
  }

  return false;
}

function detectPackageManager() {
  const entrypointDir = path.dirname(path.resolve(process.argv[1]));
  for (const startDir of new Set([codexPackageRoot, entrypointDir])) {
    const filesystemRoot = path.parse(startDir).root;
    for (
      let currentDir = startDir;
      currentDir !== filesystemRoot;
      currentDir = path.dirname(currentDir)
    ) {
      if (isPnpmOwnedInstall(path.join(currentDir, "node_modules"))) {
        return "pnpm";
      }
    }

    if (isPnpmOwnedInstall(path.join(filesystemRoot, "node_modules"))) {
      return "pnpm";
    }
  }

  const userAgent = process.env.npm_config_user_agent || "";
  if (/\bbun\//.test(userAgent)) {
    return "bun";
  }

  const execPath = process.env.npm_execpath || "";
  if (execPath.includes("bun")) {
    return "bun";
  }

  if (
    __dirname.includes(".bun/install/global") ||
    __dirname.includes(".bun\\install\\global")
  ) {
    return "bun";
  }

  return userAgent ? "npm" : null;
}

const packageManager = detectPackageManager();
const packageManagerEnvVar =
  packageManager === "bun"
    ? "CODEX_MANAGED_BY_BUN"
    : packageManager === "pnpm"
      ? "CODEX_MANAGED_BY_PNPM"
      : "CODEX_MANAGED_BY_NPM";
const env = {
  ...process.env,
  CODEX_MANAGED_PACKAGE_ROOT: codexPackageRoot,
};
delete env.CODEX_MANAGED_BY_NPM;
delete env.CODEX_MANAGED_BY_BUN;
delete env.CODEX_MANAGED_BY_PNPM;
env[packageManagerEnvVar] = "1";

const child = spawn(binaryPath, process.argv.slice(2), {
  stdio: "inherit",
  env,
});

child.on("error", (err) => {
  console.error(err);
  process.exit(1);
});

const forwardSignal = (signal) => {
  if (child.killed) {
    return;
  }
  try {
    child.kill(signal);
  } catch {
    /* ignore */
  }
};

["SIGINT", "SIGTERM", "SIGHUP"].forEach((sig) => {
  process.on(sig, () => forwardSignal(sig));
});

const childResult = await new Promise((resolve) => {
  child.on("exit", (code, signal) => {
    if (signal) {
      resolve({ type: "signal", signal });
    } else {
      resolve({ type: "code", exitCode: code ?? 1 });
    }
  });
});

if (childResult.type === "signal") {
  process.kill(process.pid, childResult.signal);
} else {
  process.exit(childResult.exitCode);
}
