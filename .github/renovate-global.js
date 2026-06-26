module.exports = {
  platform: "github",
  repositories: ["screwys/Rufin"],
  onboarding: false,
  requireConfig: "required",
  gitIgnoredAuthors: [
    "renovate[bot] <29139614+renovate[bot]@users.noreply.github.com>",
  ],
  allowedCommands: [
    "^cargo run --locked -p xtask -- generate flatpak-sources$",
  ],
};
