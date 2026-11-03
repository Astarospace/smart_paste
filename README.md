# Yazi Smart Paste

A lightning-fast, native Rust utility for the [Yazi](https://github.com/sxyazi/yazi) file manager that instantly pastes any content from your Wayland system clipboard—including `.docx` and `.pdf` documents, images, and text.


## Prerequisites

- **Linux** running a **Wayland** compositor (Sway, Hyprland, KDE Plasma Wayland, Niri, etc.)
- **Yazi** file manager installed
- **Rust & Cargo** (required to build from source)


## Installation

### Build from Source

1. **Clone the repository:**
   ```bash
   git clone [https://github.com/YOUR_USERNAME/smart_paste.git](https://github.com/YOUR_USERNAME/smart_paste.git)
   cd smart_paste
```

2. **Compile the release binary:**
  ```bash
  cargo build --release

```


3. **Install the binary to your Yazi configuration directory:**
  ```bash
  mkdir -p ~/.config/yazi
  cp target/release/smart_paste ~/.config/yazi/smart_paste_bin

```


## Configuration

Add the following keybinding to your Yazi `keymap.toml` file (typically located at `~/.config/yazi/keymap.toml`):

  ```toml
  # smart_paste script for yazi
  # For automatic renaming when a filename already exists:
  [[mgr.prepend_keymap]]
  on  = "<C-p>"
  run = "shell '~/.config/yazi/smart_paste_bin' --confirm"
  desc = "Smart paste files, documents, and images from clipboard"
  
  # For conflict dialogue:
  [[mgr.prepend_keymap]]
  on  = "<P>"
  run = "shell '~/.config/yazi/smart_paste_bin' --block --confirm"
  desc = "Smart paste files, documents, and images from clipboard"

```


## Usage

1. Copy any file, document, or image from your browser or file manager (`Ctrl+C`), or capture a screenshot.
2. Navigate to your target directory inside Yazi.
3. Press **`Ctrl + P`** to paste your clipboard contents instantly.


## License

This plugin is MIT-licensed. For more information check the [LICENSE](LICENSE) file.


