/*
 * Reference device-side server for ADB screen mirroring.
 *
 * This demonstrates the recommended scrcpy-like architecture:
 *   - MediaCodec H.264 encoding via Surface input
 *   - SurfaceControl hidden API for screen capture (shell user)
 *   - Local abstract socket for ADB tunnel transport
 *   - Separate control channel for touch/key injection
 *
 * BUILD (requires Android SDK build-tools):
 *   javac -source 1.8 -target 1.8 \
 *     -cp $ANDROID_HOME/platforms/android-34/android.jar \
 *     -d out server/Server.java
 *   d8 --output server.jar out/com/adbui/server/Server.class
 *
 * DEPLOY & RUN:
 *   adb push server.jar /data/local/tmp/mirror-server.jar
 *   adb shell CLASSPATH=/data/local/tmp/mirror-server.jar \
 *     app_process / com.adbui.server.Server [width] [height] [bitrate]
 *
 * CONNECT (from host):
 *   adb forward tcp:0 localabstract:adb-mirror  -> returns assigned port
 *   Connect to localhost:<port> for the video stream.
 *
 * PROTOCOL (video socket):
 *   Server sends:  raw H.264 Annex B byte stream (SPS/PPS + IDR + P-frames)
 *   Client sends:  control events (see ControlHandler)
 *
 * NOTES:
 *   - Runs as shell user via app_process (no APK needed)
 *   - Uses hidden SurfaceControl API via reflection
 *   - Android 14+ MediaProjection restrictions do NOT apply to shell user
 *   - For orientation changes: resize the VirtualDisplay, don't restart capture
 *   - The host Rust client (src/adb/mirror.rs) handles decoding and rendering
 */

package com.adbui.server;

import android.graphics.Rect;
import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaFormat;
import android.net.LocalServerSocket;
import android.net.LocalSocket;
import android.os.IBinder;
import android.view.Surface;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.lang.reflect.Method;
import java.nio.ByteBuffer;
import java.net.Socket;

public class Server {

    private static final String SOCKET_NAME = "adb-mirror";
    private static final int DEFAULT_WIDTH = 720;
    private static final int DEFAULT_HEIGHT = 1280;
    private static final int DEFAULT_BITRATE = 4_000_000;
    private static final int IFRAME_INTERVAL = 2; // seconds
    private static final int FPS = 30;
    private static final String DEFAULT_TRANSPORT = "forward";
    private static final int DISPLAY_STATE_ON = 2;

    private static final class CaptureDisplayInfo {
        final int displayId;
        final int width;
        final int height;
        final int rotation;
        final int layerStack;
        final int state;

        CaptureDisplayInfo(int displayId, int width, int height, int rotation, int layerStack, int state) {
            this.displayId = displayId;
            this.width = width;
            this.height = height;
            this.rotation = rotation;
            this.layerStack = layerStack;
            this.state = state;
        }
    }

    public static void main(String[] args) {
        int width = parsePositiveArg(args, 0, DEFAULT_WIDTH);
        int height = parsePositiveArg(args, 1, DEFAULT_HEIGHT);
        int bitRate = parsePositiveArg(args, 2, DEFAULT_BITRATE);
        String transport = args.length >= 4 ? args[3] : DEFAULT_TRANSPORT;

        System.err.println("[mirror-server] Starting " + width + "x" + height
                + " @ " + bitRate + " bps via " + transport);

        try {
            run(width, height, bitRate, transport, args);
        } catch (Exception e) {
            System.err.println("[mirror-server] Fatal: " + e);
            e.printStackTrace(System.err);
            System.exit(1);
        }
    }

    private static void run(int width, int height, int bitRate, String transport, String[] args)
            throws Exception {
        // 1. Configure MediaCodec encoder with Surface input
        MediaFormat format = MediaFormat.createVideoFormat(
                MediaFormat.MIMETYPE_VIDEO_AVC, width, height);
        format.setInteger(MediaFormat.KEY_BIT_RATE, bitRate);
        format.setInteger(MediaFormat.KEY_FRAME_RATE, FPS);
        format.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, IFRAME_INTERVAL);
        format.setInteger(MediaFormat.KEY_COLOR_FORMAT,
                MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface);

        MediaCodec codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC);
        codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE);
        Surface inputSurface = codec.createInputSurface();
        codec.start();

        // 2. Create a virtual display that renders to the encoder's Surface
        //    Using hidden SurfaceControl API (accessible to shell user)
        IBinder display = createDisplay();
        CaptureDisplayInfo displayInfo = resolveCaptureDisplay();
        Rect deviceRect = new Rect(0, 0, displayInfo.width, displayInfo.height);
        Rect videoRect = new Rect(0, 0, width, height);
        System.err.println("[mirror-server] Capturing displayId=" + displayInfo.displayId
                + " layerStack=" + displayInfo.layerStack
                + " size=" + displayInfo.width + "x" + displayInfo.height
                + " rotation=" + displayInfo.rotation
                + " state=" + displayInfo.state);
        setDisplaySurface(display, inputSurface, deviceRect, videoRect, displayInfo);

        System.err.println("[mirror-server] Capture started, waiting for client...");

        Thread controlThread = null;
        try {
            if ("reverse".equals(transport)) {
                int videoPort = parsePositiveArg(args, 4, 0);
                int controlPort = parsePositiveArg(args, 5, 0);
                if (videoPort <= 0 || controlPort <= 0) {
                    throw new IllegalArgumentException("reverse transport requires video/control ports");
                }

                try (Socket videoSocket = new Socket("127.0.0.1", videoPort);
                     Socket controlSocket = new Socket("127.0.0.1", controlPort);
                     OutputStream output = videoSocket.getOutputStream();
                     InputStream input = controlSocket.getInputStream()) {
                    System.err.println("[mirror-server] Reverse video/control connected");
                    controlThread = startControlThread(input);
                    streamVideo(codec, output);
                } catch (IOException e) {
                    System.err.println("[mirror-server] Reverse transport stopped: " + e.getMessage());
                }
            } else {
                String videoSocketName = args.length >= 5 ? args[4] : SOCKET_NAME + "-video";
                String controlSocketName = args.length >= 6 ? args[5] : SOCKET_NAME + "-control";

                try (LocalServerSocket videoServer = new LocalServerSocket(videoSocketName);
                     LocalServerSocket controlServer = new LocalServerSocket(controlSocketName)) {
                    System.err.println("[mirror-server] Waiting on forward sockets");
                    LocalSocket videoClient = videoServer.accept();
                    LocalSocket controlClient = controlServer.accept();
                    System.err.println("[mirror-server] Forward video/control connected");

                    try (LocalSocket ignoredVideo = videoClient;
                         LocalSocket ignoredControl = controlClient;
                         OutputStream output = videoClient.getOutputStream();
                         InputStream input = controlClient.getInputStream()) {
                        controlThread = startControlThread(input);
                        streamVideo(codec, output);
                    } catch (IOException e) {
                        System.err.println("[mirror-server] Forward transport stopped: " + e.getMessage());
                    }
                }
            }
        } finally {
            if (controlThread != null) {
                controlThread.interrupt();
            }
            codec.stop();
            codec.release();
            destroyDisplay(display);
            System.err.println("[mirror-server] Shutdown complete");
        }
    }

    private static Thread startControlThread(InputStream input) {
        Thread controlThread = new Thread(() -> handleControl(input), "control");
        controlThread.setDaemon(true);
        controlThread.start();
        return controlThread;
    }

    private static void streamVideo(MediaCodec codec, OutputStream output) throws IOException {
        MediaCodec.BufferInfo bufferInfo = new MediaCodec.BufferInfo();
        while (true) {
            int index = codec.dequeueOutputBuffer(bufferInfo, 100_000); // 100ms timeout
            if (index < 0) {
                continue;
            }

            try {
                ByteBuffer buffer = codec.getOutputBuffer(index);
                if (buffer != null && bufferInfo.size > 0) {
                    buffer.position(bufferInfo.offset);
                    buffer.limit(bufferInfo.offset + bufferInfo.size);
                    byte[] data = new byte[bufferInfo.size];
                    buffer.get(data);
                    output.write(data);
                }
            } finally {
                codec.releaseOutputBuffer(index, false);
            }
        }
    }

    // ─── Control handler ────────────────────────────────────────────────

    private static void handleControl(InputStream input) {
        /*
         * Control protocol (client -> server):
         *   [1 byte: type]
         *   [variable: data]
         *
         * Types:
         *   1 = Tap:   [4B x (BE)] [4B y (BE)]
         *   2 = Swipe: [4B x1] [4B y1] [4B x2] [4B y2] [4B duration_ms]
         *   3 = Key:   [4B keycode (BE)]
         *
         * Events are injected via the device `input` command for broad compatibility.
         */
        try {
            byte[] buf = new byte[32];
            while (true) {
                int type = input.read();
                if (type == -1) break;

                switch (type) {
                    case 1: { // Tap
                        readFully(input, buf, 8);
                        int x = readInt(buf, 0);
                        int y = readInt(buf, 4);
                        injectTap(x, y);
                        break;
                    }
                    case 2: { // Swipe
                        readFully(input, buf, 20);
                        int x1 = readInt(buf, 0);
                        int y1 = readInt(buf, 4);
                        int x2 = readInt(buf, 8);
                        int y2 = readInt(buf, 12);
                        int duration = readInt(buf, 16);
                        injectSwipe(x1, y1, x2, y2, duration);
                        break;
                    }
                    case 3: { // Key
                        readFully(input, buf, 4);
                        int keycode = readInt(buf, 0);
                        injectKey(keycode);
                        break;
                    }
                    default:
                        System.err.println("[control] Unknown event type: " + type);
                }
            }
        } catch (Exception e) {
            System.err.println("[control] Stopped: " + e.getMessage());
        }
    }

    // ─── Input injection (via shell commands as fallback) ───────────────

    private static void injectTap(int x, int y) {
        exec("input", "tap", String.valueOf(x), String.valueOf(y));
    }

    private static void injectSwipe(int x1, int y1, int x2, int y2, int duration) {
        exec("input", "swipe",
                String.valueOf(x1), String.valueOf(y1),
                String.valueOf(x2), String.valueOf(y2),
                String.valueOf(duration));
    }

    private static void injectKey(int keycode) {
        exec("input", "keyevent", String.valueOf(keycode));
    }

    private static void exec(String... cmd) {
        try {
            Process process = new ProcessBuilder(cmd)
                    .redirectErrorStream(true)
                    .start();
            process.getOutputStream().close();
            process.getInputStream().close();
        } catch (IOException e) {
            System.err.println("[input] exec failed: " + e.getMessage());
        }
    }

    // ─── Hidden API: SurfaceControl ─────────────────────────────────────
    //
    // These methods use reflection to access android.view.SurfaceControl,
    // which is not part of the public SDK but is accessible to the shell user.
    // scrcpy uses the same approach.

    @SuppressWarnings("JavaReflectionMemberAccess")
    private static IBinder createDisplay() throws Exception {
        Class<?> cls = Class.forName("android.view.SurfaceControl");
        Method method = cls.getMethod("createDisplay", String.class, boolean.class);
        return (IBinder) method.invoke(null, "adb-mirror", false);
    }

    private static void destroyDisplay(IBinder display) {
        try {
            Class<?> cls = Class.forName("android.view.SurfaceControl");
            Method method = cls.getMethod("destroyDisplay", IBinder.class);
            method.invoke(null, display);
        } catch (Exception e) {
            System.err.println("[mirror-server] destroyDisplay failed: " + e);
        }
    }

    @SuppressWarnings("JavaReflectionMemberAccess")
    private static void setDisplaySurface(IBinder display, Surface surface,
                                          Rect deviceRect, Rect videoRect,
                                          CaptureDisplayInfo displayInfo) throws Exception {
        Class<?> cls = Class.forName("android.view.SurfaceControl");

        Method openTransaction = cls.getMethod("openTransaction");
        Method closeTransaction = cls.getMethod("closeTransaction");
        Method setDisplaySurface = cls.getMethod("setDisplaySurface", IBinder.class, Surface.class);
        Method setDisplayProjection = cls.getMethod("setDisplayProjection",
                IBinder.class, int.class, Rect.class, Rect.class);
        Method setDisplayLayerStack = cls.getMethod("setDisplayLayerStack", IBinder.class, int.class);

        openTransaction.invoke(null);
        try {
            setDisplaySurface.invoke(null, display, surface);
            setDisplayProjection.invoke(null, display, displayInfo.rotation, deviceRect, videoRect);
            setDisplayLayerStack.invoke(null, display, displayInfo.layerStack);
        } finally {
            closeTransaction.invoke(null);
        }
    }

    private static CaptureDisplayInfo resolveCaptureDisplay() throws Exception {
        Class<?> cls = Class.forName("android.hardware.display.DisplayManagerGlobal");
        Method getInstance = cls.getMethod("getInstance");
        Object dmg = getInstance.invoke(null);
        Method getDisplayIds = cls.getMethod("getDisplayIds");
        Method getDisplayInfo = cls.getMethod("getDisplayInfo", int.class);

        int[] displayIds = (int[]) getDisplayIds.invoke(dmg);
        CaptureDisplayInfo best = null;
        for (int displayId : displayIds) {
            Object info = getDisplayInfo.invoke(dmg, displayId);
            if (info == null) {
                continue;
            }
            Class<?> infoCls = info.getClass();
            int width = infoCls.getField("logicalWidth").getInt(info);
            int height = infoCls.getField("logicalHeight").getInt(info);
            int rotation = infoCls.getField("rotation").getInt(info);
            int layerStack = infoCls.getField("layerStack").getInt(info);
            int state = infoCls.getField("state").getInt(info);
            if (width <= 0 || height <= 0 || layerStack < 0) {
                continue;
            }

            CaptureDisplayInfo candidate = new CaptureDisplayInfo(
                    displayId, width, height, rotation, layerStack, state);
            if (best == null || compareDisplays(candidate, best) > 0) {
                best = candidate;
            }
        }

        if (best == null) {
            throw new IllegalStateException("No suitable display found for capture");
        }

        return best;
    }

    private static int compareDisplays(CaptureDisplayInfo lhs, CaptureDisplayInfo rhs) {
        int lhsStateScore = lhs.state == DISPLAY_STATE_ON ? 1 : 0;
        int rhsStateScore = rhs.state == DISPLAY_STATE_ON ? 1 : 0;
        if (lhsStateScore != rhsStateScore) {
            return lhsStateScore - rhsStateScore;
        }

        int lhsDefaultScore = lhs.displayId == 0 ? 1 : 0;
        int rhsDefaultScore = rhs.displayId == 0 ? 1 : 0;
        if (lhsDefaultScore != rhsDefaultScore) {
            return lhsDefaultScore - rhsDefaultScore;
        }

        int lhsLayerScore = lhs.layerStack >= 0 ? 1 : 0;
        int rhsLayerScore = rhs.layerStack >= 0 ? 1 : 0;
        if (lhsLayerScore != rhsLayerScore) {
            return lhsLayerScore - rhsLayerScore;
        }

        long lhsArea = (long) lhs.width * lhs.height;
        long rhsArea = (long) rhs.width * rhs.height;
        if (lhsArea != rhsArea) {
            return lhsArea > rhsArea ? 1 : -1;
        }

        return rhs.displayId - lhs.displayId;
    }

    // ─── Utilities ──────────────────────────────────────────────────────

    private static int parsePositiveArg(String[] args, int index, int defaultValue) {
        if (index >= args.length) {
            return defaultValue;
        }

        try {
            int value = Integer.parseInt(args[index]);
            return value > 0 ? value : defaultValue;
        } catch (NumberFormatException e) {
            return defaultValue;
        }
    }

    private static void readFully(InputStream in, byte[] buf, int len) throws IOException {
        int offset = 0;
        while (offset < len) {
            int n = in.read(buf, offset, len - offset);
            if (n == -1) throw new IOException("EOF");
            offset += n;
        }
    }

    private static int readInt(byte[] buf, int offset) {
        return ((buf[offset] & 0xFF) << 24)
                | ((buf[offset + 1] & 0xFF) << 16)
                | ((buf[offset + 2] & 0xFF) << 8)
                | (buf[offset + 3] & 0xFF);
    }
}
