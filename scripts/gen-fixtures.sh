#!/usr/bin/env bash
# Regenerates the decode test fixtures: 1.0 s of a 440 Hz sine at amplitude 0.5, 44.1 kHz.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$root/crates/decode/tests/fixtures"
mkdir -p "$out"

src="sine=frequency=440:sample_rate=44100:duration=1"
# lavfi sine has a default amplitude of 1/8, so volume=4 gives the 0.5 amplitude.
# Plain "-ac 2" would upmix mono with a -3 dB pan law, so duplicate the channel explicitly.
STEREO="pan=stereo|c0=c0|c1=c0"
ff() { ffmpeg -y -hide_banner -loglevel error -f lavfi -i "$src" -af "volume=4,$AF" "$@"; }

# 8-bit PCM keeps the stereo WAV/AIFF under the 100 KB fixture limit (16-bit would be 176 KB).
AF=$STEREO ff -c:a pcm_u8 "$out/sine440.wav"
AF=$STEREO ff -c:a pcm_s8 "$out/sine440.aiff"
AF=$STEREO ff -c:a flac "$out/sine440.flac"
AF=$STEREO ff -c:a libmp3lame -b:a 128k "$out/sine440.mp3"
AF=$STEREO ff -c:a aac -b:a 128k "$out/sine440.m4a"
AF=$STEREO ff -c:a vorbis -strict -2 -b:a 128k "$out/sine440.ogg"
AF=anull ff -ac 1 "$out/sine440_mono.wav"

# Deterministic pseudo-random bytes (fixed-seed LCG), so the file is not decodable.
LC_ALL=C awk 'BEGIN { x = 12345; for (i = 0; i < 4096; i++) { x = (x * 1103515245 + 12345) % 2147483648; printf "%c", int(x / 65536) % 256 } }' \
  > "$out/corrupt.mp3"

head -c 8192 "$out/sine440.mp3" > "$out/truncated.mp3"
