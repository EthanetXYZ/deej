package deej

import (
	"bufio"
	"errors"
	"fmt"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"go.bug.st/serial"
	"go.uber.org/zap"

	"github.com/omriharel/deej/pkg/deej/util"
)

// SerialIO provides a deej-aware abstraction layer to managing serial I/O.
// it keeps trying to (re)connect in the background, so unplugging the board or starting
// deej before it's plugged in no longer requires a restart
type SerialIO struct {
	deej   *Deej
	logger *zap.SugaredLogger

	stopChannel      chan struct{}
	stopOnce         sync.Once
	reconnectChannel chan struct{}
	startOnce        sync.Once

	lock          sync.Mutex
	conn          serial.Port
	connectedPort string
	connectedBaud int
	status        ConnectionStatus

	// set when we close the connection on purpose (e.g. the COM port was changed), to skip the "disconnected" toast
	intentionalClose atomic.Bool

	// bitmask of sliders whose next read value should emit a move event even if unchanged
	resendMask atomic.Uint64

	lastKnownNumSliders        int
	currentSliderPercentValues []float32

	sliderMoveConsumers []chan SliderMoveEvent
	statusConsumers     []func(ConnectionStatus)
}

// ConnectionStatus describes the state of the serial connection, for display purposes
type ConnectionStatus struct {
	Connected bool   `json:"connected"`
	Port      string `json:"port"`
	Error     string `json:"error"`
}

// SliderMoveEvent represents a single slider move captured by deej
type SliderMoveEvent struct {
	SliderID     int
	PercentValue float32
}

const (
	reconnectInterval = 2 * time.Second
)

var expectedLinePattern = regexp.MustCompile(`^\d{1,4}(\|\d{1,4})*\r?\n$`)

// NewSerialIO creates a SerialIO instance that uses the provided deej
// instance's connection info to establish communications with the arduino chip
func NewSerialIO(deej *Deej, logger *zap.SugaredLogger) (*SerialIO, error) {
	logger = logger.Named("serial")

	sio := &SerialIO{
		deej:                deej,
		logger:              logger,
		stopChannel:         make(chan struct{}),
		reconnectChannel:    make(chan struct{}, 1),
		sliderMoveConsumers: []chan SliderMoveEvent{},
	}

	logger.Debug("Created serial i/o instance")

	// respond to config changes
	sio.setupOnConfigReload()

	return sio, nil
}

// Start begins connecting to our arduino chip in the background, retrying until it succeeds
func (sio *SerialIO) Start() {
	sio.startOnce.Do(func() {
		go sio.connectionLoop()
	})
}

// Stop signals us to shut down our serial connection, if one is active
func (sio *SerialIO) Stop() {
	sio.stopOnce.Do(func() {
		sio.logger.Debug("Shutting down serial connection")
		close(sio.stopChannel)
		sio.closeConn()
	})
}

// Status returns the current connection status
func (sio *SerialIO) Status() ConnectionStatus {
	sio.lock.Lock()
	defer sio.lock.Unlock()

	return sio.status
}

// SliderPositions returns a copy of the latest known slider positions (0.0 - 1.0, -1 if unknown)
func (sio *SerialIO) SliderPositions() []float32 {
	sio.lock.Lock()
	defer sio.lock.Unlock()

	return append([]float32(nil), sio.currentSliderPercentValues...)
}

// SubscribeToSliderMoveEvents returns an unbuffered channel that receives
// a sliderMoveEvent struct every time a slider moves
func (sio *SerialIO) SubscribeToSliderMoveEvents() chan SliderMoveEvent {
	ch := make(chan SliderMoveEvent)
	sio.sliderMoveConsumers = append(sio.sliderMoveConsumers, ch)

	return ch
}

// SubscribeToStatusChanges registers a callback that's invoked whenever the connection status changes
func (sio *SerialIO) SubscribeToStatusChanges(f func(ConnectionStatus)) {
	sio.statusConsumers = append(sio.statusConsumers, f)
}

// ResendAllSliders makes the next line read from the board emit move events for all sliders,
// which re-applies their volumes (e.g. after a sensitivity change)
func (sio *SerialIO) ResendAllSliders() {
	sio.resendMask.Store(^uint64(0))
}

// ResendSlider is like ResendAllSliders, but only re-applies a single slider's volume
func (sio *SerialIO) ResendSlider(sliderIdx int) {
	if sliderIdx < 0 || sliderIdx >= 64 {
		return
	}

	for {
		old := sio.resendMask.Load()
		if sio.resendMask.CompareAndSwap(old, old|(1<<uint(sliderIdx))) {
			return
		}
	}
}

// AvailablePorts lists the serial ports present on this machine, sorted naturally (COM2 before COM10)
func AvailablePorts() []string {
	ports, err := serial.GetPortsList()
	if err != nil {
		return nil
	}

	sort.Slice(ports, func(i, j int) bool {
		if len(ports[i]) != len(ports[j]) {
			return len(ports[i]) < len(ports[j])
		}

		return ports[i] < ports[j]
	})

	return ports
}

func (sio *SerialIO) setStatus(status ConnectionStatus) {
	sio.lock.Lock()
	changed := sio.status != status
	sio.status = status
	sio.lock.Unlock()

	if changed {
		for _, consumer := range sio.statusConsumers {
			consumer(status)
		}
	}
}

func (sio *SerialIO) stopped() bool {
	select {
	case <-sio.stopChannel:
		return true
	default:
		return false
	}
}

// connectionLoop keeps us connected for as long as deej runs
func (sio *SerialIO) connectionLoop() {
	notifiedFailure := false
	everConnected := false

	for !sio.stopped() {
		portName := sio.deej.config.ConnectionInfo.COMPort
		baudRate := sio.deej.config.ConnectionInfo.BaudRate

		sio.logger.Debugw("Attempting serial connection", "comPort", portName, "baudRate", baudRate)

		conn, err := serial.Open(portName, &serial.Mode{
			BaudRate: baudRate,
			DataBits: 8,
			StopBits: serial.OneStopBit,
			Parity:   serial.NoParity,
		})

		if err != nil {
			sio.setStatus(ConnectionStatus{Port: portName, Error: describeSerialError(err)})

			// only tell the user once per outage, we'll keep retrying quietly in the background
			if !notifiedFailure {
				sio.logger.Warnw("Failed to open serial connection, will keep retrying", "comPort", portName, "error", err)
				sio.notifyConnectionFailure(portName, err)
				notifiedFailure = true
			}

			select {
			case <-sio.stopChannel:
				return
			case <-sio.reconnectChannel:
			case <-time.After(reconnectInterval):
			}

			continue
		}

		namedLogger := sio.logger.Named(strings.ToLower(portName))
		namedLogger.Infow("Connected", "comPort", portName, "baudRate", baudRate)

		sio.lock.Lock()
		sio.conn = conn
		sio.connectedPort = portName
		sio.connectedBaud = baudRate
		sio.lastKnownNumSliders = 0
		sio.lock.Unlock()

		sio.intentionalClose.Store(false)
		sio.setStatus(ConnectionStatus{Connected: true, Port: portName})

		// let the user know things are back to normal if we previously complained, or if this is a reconnection
		if notifiedFailure || everConnected {
			sio.deej.notifier.Notify(fmt.Sprintf("Connected to %s", portName), "deej is ready to go.")
		}

		notifiedFailure = false
		everConnected = true

		// this blocks until the connection breaks or gets closed
		readErr := sio.readLines(namedLogger, conn)
		sio.closeConn()

		if sio.stopped() {
			return
		}

		if sio.intentionalClose.Load() {
			namedLogger.Info("Connection closed to apply new connection settings")
			continue
		}

		namedLogger.Warnw("Lost serial connection, will keep trying to reconnect", "error", readErr)
		sio.setStatus(ConnectionStatus{Port: portName, Error: "disconnected"})
		sio.deej.notifier.Notify(fmt.Sprintf("Lost connection to %s", portName),
			"deej will reconnect automatically once your board is back.")
		notifiedFailure = true

		select {
		case <-sio.stopChannel:
			return
		case <-sio.reconnectChannel:
		case <-time.After(reconnectInterval / 2):
		}
	}
}

func (sio *SerialIO) notifyConnectionFailure(portName string, err error) {
	var portErr *serial.PortError
	if errors.As(err, &portErr) {
		switch portErr.Code() {
		case serial.PortBusy:
			sio.deej.notifier.Notify(fmt.Sprintf("Can't connect to %s!", portName),
				"This serial port is busy, make sure to close any serial monitor or other deej instance. deej will keep retrying.")
			return
		case serial.PortNotFound:
			sio.deej.notifier.Notify(fmt.Sprintf("Can't find %s!", portName),
				"Plug in your board, or pick the right port from deej's tray menu (COM port). deej will keep retrying.")
			return
		}
	}

	sio.deej.notifier.Notify(fmt.Sprintf("Can't connect to %s!", portName),
		"deej will keep retrying. Check the logs for more details.")
}

func describeSerialError(err error) string {
	var portErr *serial.PortError
	if errors.As(err, &portErr) {
		switch portErr.Code() {
		case serial.PortBusy:
			return "port busy"
		case serial.PortNotFound:
			return "not found"
		}
	}

	return "can't connect"
}

func (sio *SerialIO) closeConn() {
	sio.lock.Lock()
	conn := sio.conn
	sio.conn = nil
	sio.lock.Unlock()

	if conn == nil {
		return
	}

	if err := conn.Close(); err != nil {
		sio.logger.Warnw("Failed to close serial connection", "error", err)
	} else {
		sio.logger.Debug("Serial connection closed")
	}
}

// requestReconnect drops the current connection (if any) so the connection loop picks up new settings right away
func (sio *SerialIO) requestReconnect() {
	sio.intentionalClose.Store(true)
	sio.closeConn()

	select {
	case sio.reconnectChannel <- struct{}{}:
	default:
	}
}

func (sio *SerialIO) setupOnConfigReload() {
	configReloadedChannel := sio.deej.config.SubscribeToChanges()

	const resendDelay = 50 * time.Millisecond

	go func() {
		for range configReloadedChannel {

			// make any config reload re-send all slider values, to ensure process volumes are being re-set
			// (with the new mapping, sensitivity and so on). this needs to happen after a small delay, because the
			// session map will also re-acquire sessions whenever the config file is reloaded, and we don't want
			// it to receive these move events while the map is still cleared
			go func() {
				<-time.After(resendDelay)
				sio.ResendAllSliders()
			}()

			sio.lock.Lock()
			connected := sio.conn != nil
			portChanged := sio.deej.config.ConnectionInfo.COMPort != sio.connectedPort ||
				sio.deej.config.ConnectionInfo.BaudRate != sio.connectedBaud
			sio.lock.Unlock()

			// if connection params have changed, drop the connection - the loop will reconnect with the new ones.
			// if we're not connected at all, retry right away since the user may have just fixed their config
			if (connected && portChanged) || !connected {
				sio.logger.Info("Connection parameters may have changed, renewing connection")
				sio.requestReconnect()
			}
		}
	}()
}

func (sio *SerialIO) readLines(logger *zap.SugaredLogger, conn serial.Port) error {
	reader := bufio.NewReader(conn)

	for {
		line, err := reader.ReadString('\n')
		if err != nil {
			if sio.deej.Verbose() {
				logger.Warnw("Failed to read line from serial", "error", err, "line", line)
			}

			return err
		}

		if sio.deej.Verbose() {
			logger.Debugw("Read new line", "line", line)
		}

		sio.handleLine(logger, line)
	}
}

func (sio *SerialIO) handleLine(logger *zap.SugaredLogger, line string) {

	// this function receives an unsanitized line which is guaranteed to end with LF,
	// but most lines will end with CRLF. it may also have garbage instead of
	// deej-formatted values, so we must check for that! just ignore bad ones
	if !expectedLinePattern.MatchString(line) {
		return
	}

	// trim the suffix
	line = strings.TrimRight(line, "\r\n")

	// split on pipe (|), this gives a slice of numerical strings between "0" and "1023"
	splitLine := strings.Split(line, "|")
	numSliders := len(splitLine)

	sio.lock.Lock()

	// update our slider count, if needed - this will send slider move events for all
	if numSliders != sio.lastKnownNumSliders {
		logger.Infow("Detected sliders", "amount", numSliders)

		sio.lastKnownNumSliders = numSliders
		sio.currentSliderPercentValues = make([]float32, numSliders)

		// reset everything to be an impossible value to force the slider move event later
		for idx := range sio.currentSliderPercentValues {
			sio.currentSliderPercentValues[idx] = -1.0
		}
	}

	// sliders we were asked to re-send get an impossible value too
	if mask := sio.resendMask.Swap(0); mask != 0 {
		for idx := range sio.currentSliderPercentValues {
			if idx < 64 && mask&(1<<uint(idx)) != 0 {
				sio.currentSliderPercentValues[idx] = -1.0
			}
		}
	}

	// for each slider:
	moveEvents := []SliderMoveEvent{}
	for sliderIdx, stringValue := range splitLine {

		// convert string values to integers ("1023" -> 1023)
		number, _ := strconv.Atoi(stringValue)

		// turns out the first line could come out dirty sometimes (i.e. "4558|925|41|643|220")
		// so let's check the numbers for correctness just in case
		if number > 1023 {
			sio.lock.Unlock()
			logger.Debugw("Got malformed line from serial, ignoring", "line", line)
			return
		}

		// map the value from raw to a "dirty" float between 0 and 1 (e.g. 0.15451...)
		dirtyFloat := float32(number) / 1023.0

		// normalize it to an actual volume scalar between 0.0 and 1.0 with 2 points of precision
		normalizedScalar := util.NormalizeScalar(dirtyFloat)

		// if sliders are inverted, take the complement of 1.0
		if sio.deej.config.InvertSliders {
			normalizedScalar = 1 - normalizedScalar
		}

		// check if it changes the desired state (could just be a jumpy raw slider value)
		if util.SignificantlyDifferent(sio.currentSliderPercentValues[sliderIdx], normalizedScalar, sio.deej.config.NoiseReductionLevel) {

			// if it does, update the saved value and create a move event
			sio.currentSliderPercentValues[sliderIdx] = normalizedScalar

			moveEvents = append(moveEvents, SliderMoveEvent{
				SliderID:     sliderIdx,
				PercentValue: normalizedScalar,
			})

			if sio.deej.Verbose() {
				logger.Debugw("Slider moved", "event", moveEvents[len(moveEvents)-1])
			}
		}
	}

	sio.lock.Unlock()

	// deliver move events if there are any, towards all potential consumers
	if len(moveEvents) > 0 {
		for _, consumer := range sio.sliderMoveConsumers {
			for _, moveEvent := range moveEvents {
				consumer <- moveEvent
			}
		}
	}
}
