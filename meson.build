project(
  'marshal',
  version: '0.1.0',
  meson_version: '>= 1.0.0',
  license: 'GPL-3.0-or-later',
)

gnome = import('gnome')

dependency('gtk4', version: '>= 4.16')
dependency('libadwaita-1', version: '>= 1.7')

install_data(
  'LICENSE',
  install_dir: get_option('datadir') / 'licenses' / 'io.github.marshal.Marshal',
)

subdir('data')
subdir('src')

gnome.post_install(
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
