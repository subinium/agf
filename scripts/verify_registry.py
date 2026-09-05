"""Verify an exact published AGF archive and clean release-commit provenance."""

import argparse
import hashlib
import io
import json
import re
import tarfile
import time
import urllib.request


def fetch(url, maximum):
    request = urllib.request.Request(url, headers={"User-Agent": "agf release verification"})
    with urllib.request.urlopen(request, timeout=30) as response:
        content = response.read(maximum + 1)
    if len(content) > maximum:
        raise RuntimeError("registry response exceeded the verification budget")
    return content


def record(version):
    records = (json.loads(line) for line in fetch("https://index.crates.io/3/a/agf", 8 * 1024 * 1024).splitlines())
    found = next((entry for entry in records if entry["vers"] == version), None)
    if found and found["yanked"]:
        raise RuntimeError(f"agf {version} is yanked")
    return found


def verify_archive(version, checksum, commit):
    archive = fetch(f"https://static.crates.io/crates/agf/agf-{version}.crate", 64 * 1024 * 1024)
    if hashlib.sha256(archive).hexdigest() != checksum:
        raise RuntimeError("published archive checksum mismatch")
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r|gz") as package:
        for member in package:
            if member.name == f"agf-{version}/.cargo_vcs_info.json":
                if not member.isfile() or member.size > 4096:
                    raise RuntimeError("invalid package provenance metadata")
                with package.extractfile(member) as stream:
                    vcs = json.load(stream)
                if vcs["git"]["sha1"] != commit or vcs["git"].get("dirty", False):
                    raise RuntimeError("published archive does not match the clean release commit")
                return
    raise RuntimeError("published archive lacks VCS provenance")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version")
    parser.add_argument("--exists-only", action="store_true")
    parser.add_argument("--expect-commit")
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", args.version):
        parser.error("use an exact X.Y.Z version")
    if not args.exists_only and not args.expect_commit:
        parser.error("--expect-commit is required for provenance verification")
    if args.expect_commit and not re.fullmatch(r"[0-9a-f]{40}", args.expect_commit):
        parser.error("--expect-commit must be a full lowercase Git SHA-1")
    for attempt in range(30 if not args.exists_only else 1):
        found = record(args.version)
        if found:
            if not args.exists_only:
                verify_archive(args.version, found["cksum"], args.expect_commit)
            print(f"verified registry agf {args.version}")
            return 0
        if not args.exists_only and attempt < 29:
            time.sleep(2)
    if args.exists_only:
        return 3
    raise RuntimeError("exact AGF version did not appear in the registry")


if __name__ == "__main__":
    raise SystemExit(main())
