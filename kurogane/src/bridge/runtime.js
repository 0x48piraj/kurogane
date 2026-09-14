(function () {

    if (window.kurogane) return; // prevent double injection

    /**
     * Convert a native rejection into an Error with a numeric .code.
     *
     * The native side sends "{code}: {message}" (e.g. "-1: handler panicked").
     * CEF rejects a promise with an Error whose message is that text; a
     * refused subscription passes it as a plain string. Either form becomes
     * an Error carrying the message after the code and a numeric .code.
     *
     * Anything else (an Error without a code, a non-string value) passes
     * through unchanged.
     */
    function toError(e) {
        const text = typeof e === 'string' ? e
            : (e !== null && typeof e === 'object' && typeof e.message === 'string') ? e.message
            : null;
        const match = text === null ? null : /^(-?\d+): ?([\s\S]*)$/.exec(text);
        if (!match) return e;
        const err = new Error(match[2]);
        err.code = parseInt(match[1], 10);
        return err;
    }

    const utf8 = new TextEncoder();

    /**
     * View bytes (Uint8Array / ArrayBuffer / ArrayBufferView) as a Uint8Array
     * without copying. Throws TypeError for anything else.
     */
    function asBytes(data, what) {
        if (data instanceof Uint8Array) return data;
        if (data instanceof ArrayBuffer) return new Uint8Array(data);
        if (ArrayBuffer.isView(data)) {
            return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
        }
        throw new TypeError(what + ': expected a string, Uint8Array, ArrayBuffer or ArrayBufferView');
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
     * If the browser refuses the subscription (code -4: the event is not
     * permitted for this origin), the subscription is removed and onError
     * receives an Error with a numeric .code. Without onError the refusal
     * is reported with console.warn.
     *
     * @param {string} eventName
     * @param {Function} callback - receives (payload) when the event fires
     * @param {Function} [onError] - receives (Error) if the subscription is refused
     * @returns {number} subscription id (pass to off() to unsubscribe)
     */
    function on(eventName, callback, onError) {
        if (typeof eventName !== 'string') {
            throw new TypeError('on: eventName must be a string');
        }
        if (typeof callback !== 'function') {
            throw new TypeError('on: callback must be a function');
        }
        if (onError !== undefined && typeof onError !== 'function') {
            throw new TypeError('on: onError must be a function');
        }
        const report = onError || function (err) {
            console.warn('kurogane.on(' + JSON.stringify(eventName) + ') refused: ' + err.message);
        };
        return window.core.on(eventName, callback, function (e) { report(toError(e)); });
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
     * A refused open rejects with an Error carrying a numeric .code
     * (-4 when the handler is not permitted for this origin).
     *
     * @param {string} handlerName - registered stream handler name
     * @param {string} [metadata] - optional metadata string
     * @returns {Promise<Stream>}
     */
    async function openStream(handlerName, metadata) {
        let id;
        try {
            id = await window.core.openStream(handlerName, metadata || '');
        } catch (e) {
            throw toError(e);
        }
        return new Stream(id);
    }

    /**
     * Capability-mediated filesystem access.
     *
     * Methods invoke the native `fs.*` command surface. File contents are
     * transferred as raw bytes; metadata uses JSON.
     *
     * Methods reject with an Error carrying a numeric .code:
     *
     * - -5: the origin has no grant carrying the required capability.
     * - -6: the path is outside the granted roots, denied by a rule, or is a link or reparse point.
     * - -7: the path is invalid.
     * - -8: readFile or writeFile exceeds the transfer limit.
     * - -2: the request is malformed.
     * - 0: an I/O error; .message contains the OS error detail.
     *
     * Paths are absolute, or relative to the origin's single allowed root.
     * ".." popping above a root is denied (-6). Symlinks, junctions and
     * other reparse points are never followed, listed or opened.
     */
    const fs = Object.freeze({
        /**
         * Read a file. Resolves with its bytes as a Uint8Array.
         * @param {string} path
         * @returns {Promise<Uint8Array>}
         */
        async readFile(path) {
            // Send the UTF-8 path; receive the file contents as raw bytes.
            const bytes = await invoke('fs.read_file', utf8.encode(path));
            return new Uint8Array(bytes);
        },

        /**
         * Write a file. Replacing an existing file needs WRITE, creating a
         * new one needs CREATE. Accepts a string (UTF-8), a Uint8Array, or
         * any ArrayBuffer / ArrayBufferView.
         * @param {string} path
         * @param {string | ArrayBuffer | ArrayBufferView} data
         */
        async writeFile(path, data) {
            // Binary frame: [path length: u32 LE][path: UTF-8][contents].
            const pathBytes = utf8.encode(path);
            const contents = typeof data === 'string'
                ? utf8.encode(data)
                : asBytes(data, 'fs.writeFile');
            const frame = new Uint8Array(4 + pathBytes.length + contents.length);
            new DataView(frame.buffer).setUint32(0, pathBytes.length, true);
            frame.set(pathBytes, 4);
            frame.set(contents, 4 + pathBytes.length);
            // An ArrayBuffer goes to the native side without another copy.
            await invoke('fs.write_file', frame.buffer);
        },

        /**
         * List a directory. Resolves with [{name, kind}] where kind is
         * one of "dir" | "file" | "other". Denied entries and links are
         * omitted by the native side.
         * @param {string} path
         * @returns {Promise<Array<{name: string, kind: string}>>}
         */
        async readDir(path) {
            const { entries } = await invoke('fs.read_dir', { path });
            return entries || [];
        },

        /**
         * Byte length of a file. A link is denied (-6), never followed.
         * @param {string} path
         * @returns {Promise<number>}
         */
        async size(path) {
            const { size } = await invoke('fs.size', { path });
            return size;
        },

        /**
         * Whether the path exists. A denied path or a link rejects (-6)
         * instead of resolving false.
         * @param {string} path
         * @returns {Promise<boolean>}
         */
        async exists(path) {
            const { exists } = await invoke('fs.exists', { path });
            return exists;
        },

        /**
         * Create a directory (parents must already exist).
         * @param {string} path
         */
        async createDir(path) {
            await invoke('fs.create_dir', { path });
        },

        /**
         * Remove a file.
         * @param {string} path
         */
        async removeFile(path) {
            await invoke('fs.remove_file', { path });
        },

        /**
         * Remove an empty directory.
         * @param {string} path
         */
        async removeDir(path) {
            await invoke('fs.remove_dir', { path });
        },

        /**
         * Rename src to dst. Needs RENAME over both paths; an existing dst
         * is replaced.
         * @param {string} src
         * @param {string} dst
         */
        async renameFile(src, dst) {
            await invoke('fs.rename_file', { src, dst });
        },

        /**
         * Copy src to dst. Needs READ on src and CREATE on dst; dst must not
         * exist. Resolves with the number of bytes copied.
         * @param {string} src
         * @param {string} dst
         * @returns {Promise<number>}
         */
        async copyFile(src, dst) {
            const { bytes } = await invoke('fs.copy_file', { src, dst });
            return bytes;
        },
    });

    window.kurogane = Object.freeze({
        invoke,
        cancel,
        on,
        off,
        openStream,
        fs,
        version: "__KUROGANE_VERSION__"
    });

})();
