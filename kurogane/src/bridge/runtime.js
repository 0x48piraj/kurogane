(function () {

    if (window.kurogane) return; // prevent double injection

    /**
     * Convert a native string rejection into a proper Error with a .code property.
     *
     * The native side sends errors in the format "{code}: {message}" (e.g. "-1: handler panicked").
     * This parses that format and creates a proper Error object.
     *
     * If the value is not a string or doesn't match the expected format, it is passed through
     * unchanged.
     */
    function toError(e) {
        if (typeof e !== 'string') return e;
        const colon = e.indexOf(':');
        if (colon < 1) return e;
        const code = parseInt(e.substring(0, colon), 10);
        if (isNaN(code)) return e;
        const err = new Error(e.substring(colon + 2).trimStart());
        err.code = code;
        return err;
    }

    /**
     * Invoke a named command.
     *
     * Accepts ArrayBuffer, ArrayBufferView, or any JSON-serializable value.
     * For ArrayBuffer/ArrayBufferView payloads, the raw bytes are sent and the
     * response is returned as an ArrayBuffer. For JSON payloads, the value is
     * serialized before sending and the response is deserialized.
     *
     * The returned promise has a .cancel() method that sends a cancellation
     * request to the handler and rejects the promise with code 0.
     *
     * @param {string} command
     * @param {ArrayBuffer | ArrayBufferView | *} payload
     * @returns {Promise<ArrayBuffer | *> & { cancel: () => boolean }}
     */
    function invoke(command, payload) {
        let p;

        if (payload instanceof ArrayBuffer) {
            p = window.core.invoke(command, payload);
        } else if (ArrayBuffer.isView(payload)) {
            const buffer = payload.buffer.slice(
                payload.byteOffset,
                payload.byteOffset + payload.byteLength,
            );
            p = window.core.invoke(command, buffer);
        } else {
            const json = payload !== undefined ? JSON.stringify(payload) : '';
            p = window.core.invoke(command, json);
        }

        const kuroganeId = p.__kurogane_id;

        let chain;

        if (payload instanceof ArrayBuffer || ArrayBuffer.isView(payload)) {
            chain = p.catch(function(e) { throw toError(e); });
        } else {
            chain = p.then(function(result) {
                try {
                    return JSON.parse(result);
                } catch (e) {
                    throw new Error("Invalid JSON response: " + result);
                }
            }, function(e) { throw toError(e); });
        }

        chain.cancel = function() {
            return !!window.core.cancel(kuroganeId);
        };

        return chain;
    }

    /**
     * Cancel a pending IPC request by its native promise id.
     *
     * Prefer promise.cancel() on the return value of invoke().
     * This function is the underlying implementation exposed for edge cases.
     *
     * @param {number} id - The native promise id
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
     * Wraps a low-level stream ID with a high-level Stream API.
     *
     * A Stream object is returned from openStream() and provides
     * a convenient interface for reading and writing stream data.
     *
     * @param {number} id - the native stream identifier
     */
    class Stream {
        constructor(id) {
            this._id = id;
            this._dataCb = null;
            this._endCb = null;
            this._errorCb = null;
            this._buffer = []; // holds chunks arriving before onData is registered

            window.core.onStreamData(id, (data) => {
                if (this._dataCb) {
                    this._dataCb(data);
                } else {
                    this._buffer.push(data);
                }
            });

            window.core.onStreamEnd(id, (result) => {
                if (this._endCb) this._endCb(result);
            });

            window.core.onStreamError(id, (msg) => {
                if (this._errorCb) this._errorCb(msg);
            });
        }

        /**
         * Register a callback for incoming data chunks.
         *
         * The callback receives an ArrayBuffer with each chunk.
         * The callback is persistent, it fires for every chunk
         * until the stream ends or errors.
         *
         * @param {Function} callback - receives (ArrayBuffer data)
         */
        onData(callback) {
            if (typeof callback !== 'function') {
                throw new TypeError('Stream.onData: callback must be a function');
            }
            this._dataCb = callback;
            // Drain any chunks that arrived before onData was registered
            const buffered = this._buffer.splice(0);
            for (const chunk of buffered) callback(chunk);
        }

        /**
         * Register a callback for stream completion.
         *
         * Fires once when the browser signals the stream is done.
         *
         * @param {Function} callback - receives (string result)
         */
        onEnd(callback) {
            if (typeof callback !== 'function') {
                throw new TypeError('Stream.onEnd: callback must be a function');
            }
            this._endCb = callback;
        }

        /**
         * Register a callback for stream errors.
         *
         * Fires once when the browser signals a stream error.
         *
         * @param {Function} callback - receives (string errorMessage)
         */
        onError(callback) {
            if (typeof callback !== 'function') {
                throw new TypeError('Stream.onError: callback must be a function');
            }
            this._errorCb = callback;
        }

        /**
         * Write a chunk of data to the stream.
         *
         * @param {ArrayBuffer | ArrayBufferView} data
         */
        write(data) {
            let buffer;

            if (data instanceof ArrayBuffer) {
                buffer = data;
            } else if (ArrayBuffer.isView(data)) {
                buffer = data.buffer.slice(
                    data.byteOffset,
                    data.byteOffset + data.byteLength,
                );
            } else {
                throw new TypeError('Stream.write: expected ArrayBuffer or ArrayBufferView');
            }

            window.core.writeStream(this._id, buffer);
        }

        /**
         * Close the stream.
         *
         * @param {string} [result] - optional result string
         */
        end(result) {
            window.core.endStream(this._id, result || '');
        }
    }

    /**
     * Open a stream to the browser process.
     *
     * Resolves with a Stream object that provides methods for
     * reading data, writing data and handling completion.
     *
     * @param {string} handlerName - registered stream handler name
     * @param {string} [metadata] - optional metadata string
     * @returns {Promise<Stream>}
     */
    async function openStream(handlerName, metadata) {
        const id = await window.core.openStream(handlerName, metadata || '');
        return new Stream(id);
    }

    window.kurogane = Object.freeze({
        invoke,
        cancel,
        on,
        off,
        openStream,
        version: "__KUROGANE_VERSION__"
    });

})();
