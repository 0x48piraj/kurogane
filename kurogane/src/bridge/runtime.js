(function () {

    if (window.kurogane) return; // prevent double injection

    /**
     * Invoke a named JSON command.
     *
     * Payload is serialized to JSON before sending and the response is deserialized.
     *
     * @param {string} command
     * @param {*} payload - any JSON-serializable value
     * @returns {Promise<*>}
     */
    async function invoke(command, payload) {
        const json = payload !== undefined ? JSON.stringify(payload) : '';
        const result = await window.core.invoke(command, json);

        try {
            return JSON.parse(result);
        } catch (e) {
            throw new Error("Invalid JSON response: " + result);
        }
    }

    /**
     * Invoke a named binary command.
     *
     * Accepts ArrayBuffer or any ArrayBufferView (Uint8Array, Float32Array, DataView, etc.)
     * The native side only understands plain ArrayBuffers so this wrapper
     * automatically converts or slices the input to a proper ArrayBuffer.
     *
     * @param {string} command
     * @param {ArrayBuffer | ArrayBufferView} data
     * @returns {Promise<ArrayBuffer>}
     */
    function invokeBinary(command, data) {
        let buffer;

        if (data instanceof ArrayBuffer) {
            buffer = data;
        } else if (ArrayBuffer.isView(data)) {
            // If the input is a typed array or DataView, we cannot just pass its
            // underlying buffer directly because it may start at a non-zero offset.

            // Slice the buffer to get exactly the bytes this view represents.
            buffer = data.buffer.slice(
                data.byteOffset,
                data.byteOffset + data.byteLength,
            );
        } else {
            return Promise.reject(new TypeError(`invokeBinary: expected ArrayBuffer or ArrayBufferView, got ${data === null ? 'null' : typeof data}`));
        }

        return window.core.invokeBinary(command, buffer);
    }

    /**
     * Cancel a pending IPC request by its promise id.
     *
     * @param {number} id - The promise id returned by invoke/invokeBinary
     * @returns {boolean} true if the promise was found and canceled
     */
    function cancel(id) {
        return !!window.core.cancel(id);
    }

    /**
     * Subscribe to a browser-side event.
     *
     * @param {string} eventName
     * @param {Function} callback - receives (payload) when the event fires
     * @returns {number} subscription id (pass to off() to unsubscribe)
     */
    function on(eventName, callback) {
        if (typeof eventName !== 'string') {
            throw new TypeError('on: eventName must be a string');
        }
        if (typeof callback !== 'function') {
            throw new TypeError('on: callback must be a function');
        }
        return window.core.on(eventName, callback);
    }

    /**
     * Unsubscribe from an event.
     *
     * @param {number} id - subscription id returned by on()
     * @returns {boolean} true if the subscription was found and removed
     */
    function off(id) {
        return !!window.core.off(id);
    }

    /**
     * Open a stream to the browser process.
     *
     * @param {string} handlerName - registered stream handler name
     * @param {string} [metadata] - optional metadata string
     * @returns {Promise<number>} resolves with the stream id
     */
    function openStream(handlerName, metadata) {
        return window.core.openStream(handlerName, metadata || '');
    }

    /**
     * Write a chunk of data to an open stream.
     *
     * @param {number} streamId
     * @param {ArrayBuffer | ArrayBufferView} data
     */
    function writeStream(streamId, data) {
        let buffer;

        if (data instanceof ArrayBuffer) {
            buffer = data;
        } else if (ArrayBuffer.isView(data)) {
            buffer = data.buffer.slice(
                data.byteOffset,
                data.byteOffset + data.byteLength,
            );
        } else {
            throw new TypeError('writeStream: expected ArrayBuffer or ArrayBufferView');
        }

        window.core.writeStream(streamId, buffer);
    }

    /**
     * Close a stream.
     *
     * @param {number} streamId
     * @param {string} [result] - optional result string
     */
    function endStream(streamId, result) {
        window.core.endStream(streamId, result || '');
    }

    /**
     * Register a callback for incoming data chunks on a stream.
     *
     * The callback receives an ArrayBuffer with each chunk.
     * Callbacks are persistent - they fire for every chunk until the stream ends or errors.
     *
     * @param {number} streamId
     * @param {Function} callback - receives (ArrayBuffer data)
     */
    function onStreamData(streamId, callback) {
        window.core.onStreamData(streamId, callback);
    }

    /**
     * Register a callback for stream completion.
     *
     * Fires once when the browser signals the stream is done.
     *
     * @param {number} streamId
     * @param {Function} callback - receives (string result)
     */
    function onStreamEnd(streamId, callback) {
        window.core.onStreamEnd(streamId, callback);
    }

    /**
     * Register a callback for stream errors.
     *
     * Fires once when the browser signals a stream error.
     *
     * @param {number} streamId
     * @param {Function} callback - receives (string errorMessage)
     */
    function onStreamError(streamId, callback) {
        window.core.onStreamError(streamId, callback);
    }

    window.kurogane = Object.freeze({
        invoke,
        invokeBinary,
        cancel,
        on,
        off,
        openStream,
        writeStream,
        endStream,
        onStreamData,
        onStreamEnd,
        onStreamError,
        version: "0.0.5"
    });

})();
