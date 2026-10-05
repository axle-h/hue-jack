# hue-jack

Sync Philips Hue lights to whatever you're playing. A small appliance (Fedora bootc on an Intel NUC) acts as a Bluetooth speaker, AirPlay 2 receiver and YouTube Music cast target. It analyses the audio, drives a Hue Entertainment area over the local DTLS streaming API, and plays the music through its own output **delayed by the light latency**, so sound and light land together.

Everything stays on the LAN.

See [`docs/PLAN.md`](docs/PLAN.md) for the design.
