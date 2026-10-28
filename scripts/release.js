// Bump version, commit, tag, build every artifact, stage them in dist/<tag>/
// and publish the GitHub Release from there (Phase 5.1).
// Usage: pnpm release 1.0.0-beta.1   (node scripts/release.js <version>)
//
// What ends up in dist/<tag>/ (and in the release), with fixed names so
// `releases/latest/download/<name>` links keep working across versions:
//   Trak-setup.exe            Windows installer (NSIS)
//   Trak-portable.exe         the same app, no installer: keeps its data next to itself
//   Trak.apk                  Android
//   trak-server-windows.exe   server
//   trak-server-linux         server, static (musl)
//
// The server's Docker image isn't built here: crates/server/docker-compose.yml
// builds it on the server from the `stable` branch, which this script moves to
// the new tag once the release is published.
//
// If anything fails after the tag is pushed (build, missing artifact, gh
// release), the tag is deleted locally and on origin so a retry starts
// clean. The version-bump commit is never rolled back — it's harmless on
// its own and reverting it automatically risks reverting other people's
// pushes in between.
const { execSync } = require("child_process");
const fs = require("fs");
const path = require("path");

const version = process.argv[2];
if (!version || !/^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$/.test(version)) {
  console.error("Usage: pnpm release <version>  (e.g. 1.0.0-beta.1)");
  process.exit(1);
}

const root = path.resolve(__dirname, "..");
const isPrerelease = version.includes("-");
const tag = `v${version}`;
const dist = path.join(root, "dist", tag);

function run(cmd) {
  console.log(`\n$ ${cmd}`);
  execSync(cmd, { cwd: root, stdio: "inherit" });
}

function localTagExists() {
  return execSync("git tag -l", { cwd: root })
    .toString()
    .split("\n")
    .includes(tag);
}

function rollbackTag() {
  console.error(`\nRolling back tag ${tag}...`);
  if (localTagExists()) {
    try {
      run(`git tag -d ${tag}`);
    } catch (e) {
      console.error(`Could not delete local tag: ${e.message}`);
    }
  }
  try {
    run(`git push origin :refs/tags/${tag}`);
  } catch (e) {
    console.error(`Could not delete remote tag (may not have been pushed yet): ${e.message}`);
  }
}

/// The newest file under `dir` (recursively) whose name passes `test`.
function newest(dir, test) {
  let best = null;
  const walk = (d) => {
    if (!fs.existsSync(d)) return;
    for (const e of fs.readdirSync(d, { withFileTypes: true })) {
      const p = path.join(d, e.name);
      if (e.isDirectory()) walk(p);
      else if (test(e.name, p)) {
        const t = fs.statSync(p).mtimeMs;
        if (!best || t > best.t) best = { p, t };
      }
    }
  };
  walk(dir);
  if (!best) throw new Error(`Nothing matching in ${dir}`);
  return best.p;
}

function stage(from, name) {
  if (!fs.existsSync(from)) throw new Error(`Missing artifact: ${from}`);
  const to = path.join(dist, name);
  fs.copyFileSync(from, to);
  console.log(`  ${name}  <-  ${path.relative(root, from)}`);
  return to;
}

// 1. Refuse to run with uncommitted changes, so the release commit is clean.
const status = execSync("git status --porcelain", { cwd: root }).toString().trim();
if (status) {
  console.error("Working tree not clean. Commit or stash changes first:\n" + status);
  process.exit(1);
}

if (localTagExists()) {
  console.error(`Tag ${tag} already exists locally. Delete it first if you want to redo this release: git tag -d ${tag}`);
  process.exit(1);
}

// 2. Bump the version everywhere (app, Android, server), and the lockfile
// entries of our own crates with it, so the builds below don't leave the
// tree dirty.
run(`node scripts/bump-version.js ${version}`);
run(`cargo update --workspace`);

// 3. Commit (if the bump actually changed anything) + push, before tagging.
run(`git add -A`);
const staged = execSync("git status --porcelain", { cwd: root }).toString().trim();
if (staged) {
  run(`git commit -m "chore: release ${tag}"`);
} else {
  console.log("\nVersion already at the target value, nothing to commit.");
}
run(`git push`);

// 4. Tag + push the tag. Everything from here on rolls the tag back on failure.
run(`git tag ${tag}`);
run(`git push origin ${tag}`);

try {
  // 5. Build everything.
  const started = Date.now();
  run(`pnpm build:win`);
  run(`pnpm build:android`);
  run(`pnpm server:build-win`);
  run(`pnpm server:build-linux`);

  // 6. Stage into dist/<tag>/. Only files this run produced: an old APK or
  // installer left in the build folders must not ship under the new tag.
  const fresh = (p) => fs.statSync(p).mtimeMs >= started;
  fs.rmSync(dist, { recursive: true, force: true });
  fs.mkdirSync(dist, { recursive: true });
  console.log(`\nStaging in ${path.relative(root, dist)}:`);
  const files = [
    stage(
      newest(path.join(root, "target/release/bundle/nsis"), (n, p) => n.endsWith("-setup.exe") && n.includes(version) && fresh(p)),
      "Trak-setup.exe"
    ),
    stage(path.join(root, "target/release/trak.exe"), "Trak-portable.exe"),
    stage(
      newest(path.join(root, "app/src-tauri/gen/android/app/build/outputs/apk"), (n, p) => n.endsWith("-release.apk") && fresh(p)),
      "Trak.apk"
    ),
    stage(path.join(root, "target/release/trak-server.exe"), "trak-server-windows.exe"),
    stage(path.join(root, "target/x86_64-unknown-linux-musl/release/trak-server"), "trak-server-linux"),
  ];

  // 7. Publish the GitHub Release from the staged files.
  const prereleaseFlag = isPrerelease ? "--prerelease" : "";
  run(
    `gh release create ${tag} ` +
      files.map((f) => `"${f}"`).join(" ") +
      ` --title "Trak ${tag}" ${prereleaseFlag} --notes "Trak ${tag}"`
  );

  // 8. Point `stable` at this release: it's what the server's
  // docker-compose.yml builds by default. Only now, so a failed release never
  // reaches the servers.
  // The release is already public: a failure here must not roll the tag back.
  try {
    const sha = execSync(`git rev-list -n 1 ${tag}`, { cwd: root }).toString().trim();
    run(`git push --force origin ${sha}:refs/heads/stable`);
  } catch (e) {
    console.error(`\nCould not move stable to ${tag}: ${e.message}`);
    console.error(`Do it by hand: git push --force origin ${tag}^{commit}:refs/heads/stable`);
  }

  console.log(`\nDone. Release ${tag} published from ${path.relative(root, dist)}.`);
} catch (e) {
  console.error(`\nRelease step failed: ${e.message}`);
  rollbackTag();
  console.error(
    "\nTag rolled back. The version bump/commit already pushed is left as-is - fix the issue and rerun."
  );
  process.exit(1);
}
