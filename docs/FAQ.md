# deej FAQ

Answers for building a deej box and using deej on Windows. Something missing? Open an issue.

- [Hardware](#hardware)
- [Arduino and the sketch](#arduino-and-the-sketch)
- [First run](#first-run)
- [Mapping apps to sliders](#mapping-apps-to-sliders)
- [Everyday use](#everyday-use)
- [Coming from the original deej](#coming-from-the-original-deej)

## Hardware

### What do I need?

An Arduino-compatible board with a USB serial connection (an Uno, Nano or Pro Micro all work), one potentiometer per
control, and some wire. A case is optional; there are many printable designs for deej online.

### Sliders or knobs?

Either, or a mix. Electrically they're the same thing: a potentiometer. Sliders show their position at a glance; knobs
take less space. deej doesn't know or care which you used.

### What potentiometers should I buy?

**Linear** ones (often marked "B", like B10K). Logarithmic ("A") pots put most of the range at one end, which feels
wrong with deej's own response curves. 10 kΩ is the usual choice; anything from about 1 kΩ to 100 kΩ works.

### How do I wire them?

Each potentiometer has three pins:

1. One outer pin to **5V** (or 3.3V on 3.3 V boards).
2. The other outer pin to **GND**.
3. The middle pin (the wiper) to an **analog input**: A0 for the first control, A1 for the second, and so on.

All the 5V pins can share one wire, and all the GND pins another. If a control turns the wrong way, swap its two outer
pins, or see [inverted controls](#one-control-works-backwards-or-all-of-them-do).

## Arduino and the sketch

### I've never used an Arduino. Where do I start?

Install the [Arduino IDE](https://www.arduino.cc/en/software), plug the board in, and pick your board and port under
Tools. Uploading the "Blink" example (File → Examples → Basics) is a good first test.

### Which sketch do I upload?

[`arduino/deej-5-sliders-vanilla`](../arduino/deej-5-sliders-vanilla/deej-5-sliders-vanilla.ino). Before uploading,
set `NUM_SLIDERS` to your number of controls and list their pins in `analogInputs`. Any sketch that prints lines like
`512|1023|0` at 9600 baud works too; deej Next uses the same protocol as the original deej.

### How do I find the board's COM port?

Open deej → **Hardware**. The port list marks USB boards and shows their names. Windows' Device Manager (Ports
(COM & LPT)) shows the same thing. The number can change when you use a different USB port.

### How do I check the board is sending data?

deej → **Hardware** shows the raw lines as they arrive and a meter per control. Moving a control should change one
number between 0 and 1023. The Arduino IDE's Serial Monitor (9600 baud) shows the same lines, but close it before
starting deej: only one program can use the port at a time.

## First run

### deej says the port is "in use by another program"

Something else has the port open: the Arduino IDE's Serial Monitor, the original deej, or a second copy. Close it and
deej connects by itself within a couple of seconds.

### deej is connected but nothing happens when I move a control

Check **Hardware**:

- **No lines at all:** some boards (Pro Micro, RP2040, many ESP32s) only send data when DTR is set. Turn on Settings →
  Advanced → Assert DTR.
- **Garbled text:** the baud rate doesn't match the sketch's `Serial.begin(...)`.
- **Lines arrive, numbers change:** the control is working; its slider just isn't mapped to anything yet. See
  [mapping](#mapping-apps-to-sliders).

### Volumes jump around while nobody touches anything

Cheap potentiometers and long wires add a little noise. Raise **Settings → Hardware → Noise reduction** (Low ignores
changes under 2 %, Default 3 %, High 4 %).

### One control works backwards, or all of them do

All of them: **Settings → Hardware → Invert sliders**. Just one: swap its two outer pins.

### A control only reaches about half volume

The potentiometer probably gets 3.3 V while the board reads against 5 V. Wire it to 5V. Also check that slider's
**max volume** on the Sliders page; it may be set below 100 %.

## Mapping apps to sliders

### How do I map an app?

On the **Sliders** page, open a slider's card and add a target. The picker lists the apps playing sound right now,
your audio devices, and the special targets below. You can also type any program name, such as `spotify.exe`, before
the app is running. Changes save to `config.yaml` straight away.

### Where's config.yaml, and can I edit it by hand?

It's next to `deej-next.exe` (Settings → General shows the exact path and opens it in Notepad). Hand edits are picked
up as soon as you save. If the file has a mistake, deej keeps the previous settings and shows the problem with its
line number under Settings → Diagnostics.

### How do I find an app's program name?

The picker on the Sliders page shows the apps that are playing sound. Otherwise open Task Manager → Details while the
app runs; the Name column has it (for example `Discord.exe`). Capitalisation doesn't matter.

### What special targets are there?

| Target | Controls |
|---|---|
| `master` | the default playback device |
| `mic` | the default recording device |
| `system` | Windows' system sounds |
| `deej.current` | whichever app is in the foreground |
| `deej.unmapped` | every app no other slider names (apps listed under Settings → Audio are left alone) |
| a device name | that device, for example `Speakers (Realtek(R) Audio)` |

### Can I put all my games on one slider without listing them?

Yes: map that slider to `deej.unmapped` ("All other apps"). It controls everything that isn't on another slider. Add
anything it shouldn't touch, such as a music player, under **Settings → Audio**.

### Volume changes too fast at the low end

Give the slider a **response curve** (Sliders page, or Settings → Slider mappings for all sliders). A curve above 1
spreads the low volumes over more of the slider's travel. "Balanced" (2) turns 10 % slider into 1 % volume and 50 %
into 25 %. The graph shows where your slider is on the curve as you move it.

## Everyday use

### How do I start deej with Windows?

Settings → General → **Start with Windows**. deej then starts in the tray when you sign in.

### Where did deej go when I closed the window?

It keeps running in the tray (click the ^ arrow next to the clock if the icon is hidden). Click the icon to open the
window; right-click it for mute, playback device, restart board connection, and Quit.

### Does deej survive sleep, unplugging the board, or changing audio devices?

Yes. After sleep it reconnects the board and re-scans audio. An unplugged board is picked up again when it's plugged
back in, and a new default device gets the master slider's volume.

### Something isn't working and none of this helped

Settings → Diagnostics → **Open logs folder**. `deej-latest-run.log` usually says what's wrong. Starting deej with
`--verbose` adds detail. Please attach it if you open an issue.

## Coming from the original deej

Your board, sketch and `config.yaml` work as they are. Quit the original deej, put your `config.yaml` next to
`deej-next.exe`, and start it. The [README](../README.md#moving-over-from-the-original-deej) has the details,
including how to switch Start with Windows over.
