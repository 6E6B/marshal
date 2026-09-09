cargo_script = find_program('../build-aux/cargo.sh')

custom_target(
  'marshal',
  build_by_default: true,
  build_always_stale: true,
  output: 'marshal',
  console: true,
  install: true,
  install_dir: get_option('bindir'),
  command: [
    cargo_script,
    meson.project_source_root(),
    meson.project_build_root(),
    '@OUTPUT@',
    'release',
    '--manifest-path', meson.project_source_root() / 'Cargo.toml',
    '--release',
  ],
)
