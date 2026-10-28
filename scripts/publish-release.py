"""Publish already-built VIBE installers. Credentials stay in Git Credential Manager."""
import argparse
import base64
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import subprocess
import urllib.error
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
REPOSITORY = "Szilaay/Vibe"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--publish", action="store_true", help="Publish after all artifacts upload successfully")
    options = parser.parse_args()
    version = re.search(r'^version = "(\d+\.\d+\.\d+)"', (ROOT / "Cargo.toml").read_text(), re.M)[1]
    installer = ROOT / "dist" / f"VIBE-Setup-{version}.exe"
    manifest = ROOT / "dist/latest.json"
    assert installer.is_file() and manifest.is_file(), "Run scripts/installer.ps1 first"
    verified = subprocess.run([str(ROOT / "target/release/vibe-release.exe"), "verify", str(manifest), str(installer)], capture_output=True, text=True, timeout=30)
    assert verified.returncode == 0, "Release signature or installer verification failed"
    envelope = json.loads(manifest.read_text())
    payload = json.loads(base64.b64decode(envelope["payload"]))
    assert payload["version"] == version and payload["size"] == installer.stat().st_size
    with installer.open("rb") as stream:
        checksum = hashlib.file_digest(stream, "sha256").hexdigest()
    assert payload["sha256"] == checksum, "Installer changed after signing"
    assert payload["url"] == f"https://github.com/{REPOSITORY}/releases/download/v{version}/{installer.name}"

    env = os.environ.copy()
    env.update(GIT_TERMINAL_PROMPT="0", GCM_INTERACTIVE="Never")
    git = Path(os.environ.get("ProgramFiles", r"C:\Program Files")) / "Git/cmd/git.exe"
    result = subprocess.run([str(git), "credential", "fill"], input="protocol=https\nhost=github.com\n\n", text=True, capture_output=True, env=env, timeout=15)
    assert result.returncode == 0, "Connect a GitHub account using Git Credential Manager first"
    fields = dict(line.split("=", 1) for line in result.stdout.splitlines() if "=" in line)
    headers = {"Authorization": "Bearer " + fields["password"], "Accept": "application/vnd.github+json", "User-Agent": "VIBE-release-publisher"}

    def api(path, data=None, method=None):
        request = urllib.request.Request("https://api.github.com" + path, data=None if data is None else json.dumps(data).encode(), headers=headers, method=method or ("GET" if data is None else "POST"))
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)

    repo = api("/repos/" + REPOSITORY)
    assert not repo["private"] and repo["permissions"]["push"], "Public release repository write access required"
    try:
        release = api(f"/repos/{REPOSITORY}/releases/tags/v{version}")
        assert release["draft"], "This version is already published; increment Cargo.toml instead of replacing it"
        assert not release["assets"], "Draft already has assets; inspect it before retrying publication"
    except urllib.error.HTTPError as error:
        if error.code != 404:
            raise
        release = api(f"/repos/{REPOSITORY}/releases", {"tag_name": "v" + version, "name": "VIBE " + version, "body": (ROOT / "release/notes.md").read_text(encoding="utf-8"), "draft": True, "prerelease": False})
    assert release["draft"]

    def upload(path):
        upload_url = release["upload_url"].split("{")[0]
        url = urllib.parse.urlsplit(upload_url)
        assert url.scheme == "https" and url.hostname == "uploads.github.com"
        connection = http.client.HTTPSConnection(url.hostname, timeout=300)
        connection.putrequest("POST", url.path + "?" + urllib.parse.urlencode({"name": path.name}))
        for key, value in headers.items():
            connection.putheader(key, value)
        connection.putheader("Content-Type", "application/octet-stream")
        connection.putheader("Content-Length", str(path.stat().st_size))
        connection.endheaders()
        sent = 0
        with path.open("rb") as stream:
            while chunk := stream.read(1024 * 1024):
                connection.send(chunk)
                sent += len(chunk)
                if sent % (64 * 1024 * 1024) == 0:
                    print(f"Uploading {path.name}: {sent // (1024 * 1024)} MiB", flush=True)
        response = connection.getresponse()
        data = json.load(response)
        assert response.status == 201, f"GitHub upload failed: HTTP {response.status}"
        assert data["size"] == path.stat().st_size and data["state"] == "uploaded"
        connection.close()
        print("Uploaded: " + path.name, flush=True)

    upload(installer)
    upload(manifest)
    public_key = ROOT / "release/update-public-key.txt"
    upload(public_key)
    # README and source are maintained through Git, independently of binary releases.
    if options.publish:
        release = api(f"/repos/{REPOSITORY}/releases/{release['id']}", {"draft": False, "make_latest": "true"}, "PATCH")
        print("Published: " + release["html_url"], flush=True)
    else:
        print("Draft release uploaded; publish it after review.", flush=True)


if __name__ == "__main__":
    main()
