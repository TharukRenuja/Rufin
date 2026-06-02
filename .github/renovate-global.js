module.exports = {
  platform: "github",
  repositories: ["screwys/Rufin"],
  onboarding: false,
  requireConfig: "required",
  gitIgnoredAuthors: [
    "renovate[bot] <29139614+renovate[bot]@users.noreply.github.com>",
  ],
  allowedCommands: [
    "^bash packaging/flatpak/update-cargo-sources\\.sh$",
  ],
};
