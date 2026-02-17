service files (launchd + systemd)
==================================

status: design
created: 2026-02-16
affects: deploy/ (new directory)


overview
--------

the spec says the daemon auto-starts on login via launchd
(macOS) or systemd (Linux). currently there are no service
files. users must manually run ambient-fsd start.


launchd plist (macOS)
----------------------

  deploy/com.ambient-fs.daemon.plist:

  <?xml version="1.0" encoding="UTF-8"?>
  <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
    "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
  <plist version="1.0">
  <dict>
    <key>Label</key>
    <string>com.ambient-fs.daemon</string>

    <key>ProgramArguments</key>
    <array>
      <string>/usr/local/bin/ambient-fsd</string>
      <string>start</string>
      <string>--foreground</string>
    </array>

    <key>RunAtLoad</key>
    <true/>

    <key>KeepAlive</key>
    <true/>

    <key>StandardOutPath</key>
    <string>/tmp/ambient-fs.stdout.log</string>

    <key>StandardErrorPath</key>
    <string>/tmp/ambient-fs.stderr.log</string>

    <key>EnvironmentVariables</key>
    <dict>
      <key>RUST_LOG</key>
      <string>info</string>
    </dict>
  </dict>
  </plist>

  install:
    cp deploy/com.ambient-fs.daemon.plist ~/Library/LaunchAgents/
    launchctl load ~/Library/LaunchAgents/com.ambient-fs.daemon.plist

  uninstall:
    launchctl unload ~/Library/LaunchAgents/com.ambient-fs.daemon.plist
    rm ~/Library/LaunchAgents/com.ambient-fs.daemon.plist

  notes:
    - uses --foreground because launchd manages the process
    - KeepAlive restarts on crash
    - RunAtLoad starts on login
    - log paths in /tmp (or ~/.local/share/ambient-fs/logs/)


systemd user service (Linux)
------------------------------

  deploy/ambient-fsd.service:

  [Unit]
  Description=Ambient Filesystem Awareness Daemon
  After=default.target

  [Service]
  Type=simple
  ExecStart=/usr/local/bin/ambient-fsd start --foreground
  Restart=on-failure
  RestartSec=5
  Environment=RUST_LOG=info

  [Install]
  WantedBy=default.target

  install:
    mkdir -p ~/.config/systemd/user/
    cp deploy/ambient-fsd.service ~/.config/systemd/user/
    systemctl --user daemon-reload
    systemctl --user enable ambient-fsd
    systemctl --user start ambient-fsd

  uninstall:
    systemctl --user stop ambient-fsd
    systemctl --user disable ambient-fsd
    rm ~/.config/systemd/user/ambient-fsd.service
    systemctl --user daemon-reload

  notes:
    - Type=simple because --foreground keeps process in foreground
    - Restart=on-failure auto-restarts on crash
    - user service (not system), no root needed


CLI install/uninstall commands
-------------------------------

  ambient-fsd install-service
    detects OS (macOS vs Linux)
    copies appropriate service file
    enables and starts the service
    prints status

  ambient-fsd uninstall-service
    stops service
    disables service
    removes service file

  implementation in crates/ambient-fsd/src/main.rs:
    Command::InstallService -> cmd_install_service()
    Command::UninstallService -> cmd_uninstall_service()

  cmd_install_service():
    #[cfg(target_os = "macos")]
    - write plist to ~/Library/LaunchAgents/
    - run: launchctl load <path>
    #[cfg(target_os = "linux")]
    - write service to ~/.config/systemd/user/
    - run: systemctl --user daemon-reload
    - run: systemctl --user enable ambient-fsd
    - run: systemctl --user start ambient-fsd

  the service files are embedded in the binary via
  include_str!() so no separate file distribution needed.


binary location
----------------

  the service files assume /usr/local/bin/ambient-fsd.

  cargo install ambient-fsd installs to ~/.cargo/bin/.
  the install-service command should detect the actual
  binary path and write it into the service file:

    let binary = std::env::current_exe()?;
    let plist = PLIST_TEMPLATE.replace("{BINARY}", &binary.display().to_string());


test strategy
-------------

unit tests:
  - plist template renders correctly
  - systemd template renders correctly
  - binary path substitution works
  - OS detection picks correct service type

integration tests:
  - install-service creates file in correct location
  - uninstall-service removes file
  - skip actual launchctl/systemctl calls in tests
    (mock the command execution)


depends on
----------

  - --foreground flag (done, f2q)
  - DaemonConfig (in progress)
  - binary built and accessible
