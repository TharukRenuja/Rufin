# rufin-git AUR package

This directory contains the `PKGBUILD` and initial `.SRCINFO` for the Arch User
Repository `rufin-git` package. It builds the current `main` branch from GitHub
and installs the `rufin` binary, desktop file, AppStream metadata, icon, and
compiled translations when `po/*.po` files are present.

To test it on Arch:

```bash
makepkg -si
```

To publish or update the AUR package:

```bash
git clone ssh://aur@aur.archlinux.org/rufin-git.git .local/aur/rufin-git
cp packaging/aur/rufin-git/PKGBUILD packaging/aur/rufin-git/.SRCINFO .local/aur/rufin-git/
cd .local/aur/rufin-git
makepkg --printsrcinfo > .SRCINFO
git add PKGBUILD .SRCINFO
git commit -m "Update package"
git push
```

For a future fixed-release `rufin` package, reuse the install logic but switch
`source` to a tagged release archive, replace `sha256sums`, remove `git` from
`makedepends`, and remove the `pkgver()` function.
