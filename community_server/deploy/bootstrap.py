"""Install one hash-locked pure Python wheel without changing system Python."""
import hashlib
import pathlib
import sys
import urllib.request
import zipfile
import io

url = "https://files.pythonhosted.org/packages/fe/85/7522a52e5e2f42faf1a129113ab63e548c42e103e9af395b7bfe65e403e2/gunicorn-26.2.0-py3-none-any.whl"
expected = "bd249d0b3f7972f7432f0a6b6ff3b3ee2d129f70cd1ff6c09a9dd9e29a2b88e3"
with urllib.request.urlopen(url, timeout=30) as response:
    data = response.read(1024 * 1024)
if hashlib.sha256(data).hexdigest() != expected:
    raise SystemExit("Gunicorn checksum mismatch")
root = pathlib.Path(sys.prefix).resolve()
site = root / "lib" / f"python{sys.version_info.major}.{sys.version_info.minor}" / "site-packages"
site.mkdir(parents=True, exist_ok=True)
with zipfile.ZipFile(io.BytesIO(data)) as archive:
    for name in archive.namelist():
        path = pathlib.PurePosixPath(name)
        if path.is_absolute() or ".." in path.parts or "\\" in name or not path.parts[0].startswith(("gunicorn",)):
            raise SystemExit("Unexpected wheel path")
    archive.extractall(site)
launcher = root / "bin/gunicorn"
launcher.write_text("#!/bin/sh\nexec '" + str(root / "bin/python") + "' -m gunicorn \"$@\"\n")
launcher.chmod(0o755)
print("Installed hash-verified Gunicorn in dedicated environment")
