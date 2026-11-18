# SDO Arrowgene

SDO Arrowgene aims to faithfully recreate the core rhythm gameplay of the *Super Dancer Online* series. It supports **four-key mode only**.

| Feature | What you can do |
| --- | --- |
| Single player | Play songs offline at your chosen difficulty. |
| Multiplayer | Host a room or join friends for a round. |
| Dedicated hosting | Run a standalone server with multiple rooms. |
| Wardrobe | Customize your dancer’s outfit in the **Character** tab. |

> [!IMPORTANT]
> **No game assets are included.** Each player needs a compatible SDO asset folder.

To get started, download the [client for your platform](https://github.com/sebastian-heinz/sdo-arrowgene/releases/latest), extract it, and open `sdo-arrowgene`. In **Settings → Asset Folder**, select your folder and click **Reload assets**.

## Single player

Open **Single Player**, choose a song and difficulty, and press <kbd>Enter</kbd> to play offline. Use <kbd>D</kbd> <kbd>F</kbd> <kbd>J</kbd> <kbd>K</kbd> or the arrow keys to hit notes in the four lanes.

| Four-key gameplay | Character wardrobe |
| :---: | :---: |
| ![Four-key gameplay during Holy Knight](docs/screenshots/gameplay.png) | ![A dancer previewing hairstyles in the wardrobe](docs/screenshots/wardrobe.png) |

## Multiplayer

- **Host a room** from the client under **Multiplayer → Host Server**.
- **Join a room** through **Multiplayer → Join Server**. Enter the host’s address, such as `192.168.1.20:32100` on a LAN.

Each player needs the same song and matching chart in their local asset folder.

![Two players in a multiplayer room before a song](docs/screenshots/multiplayer-room.png)

## Docker hosting

Run the standalone server to host multiple rooms. It listens on TCP port `32100` and runs without game assets.

```sh
docker run --rm --init -p 32100:32100 ghcr.io/sebastian-heinz/sdo-server:latest
```

Open **Multiplayer → Join Server** and enter `localhost:32100` if you’re playing on the server machine. From another computer, enter the host’s IP address or hostname followed by `:32100`. Once connected, create or join a room.

![Docker port mapping and startup output from the standalone server](docs/screenshots/server.png)

For a local build, run `docker build -t sdo-server .` and use `sdo-server` as the image name.
