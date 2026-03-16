/**
 * Invoke a named JSON command.
 * Payload is serialized to JSON before sending and the response is deserialized.
 *
 * @param {string} command
 * @param {*} payload - any JSON-serializable value
 * @returns {Promise<*>}
 */
export async function invoke(command, payload) {
    const json   = payload !== undefined ? JSON.stringify(payload) : '';
    const result = await window.core.invoke(command, json);
    return JSON.parse(result);
}

/**
 * Invoke a named binary command.
 *
 * Accepts ArrayBuffer or any ArrayBufferView (Uint8Array, Float32Array, DataView, etc.)
 * The native side only understands plain ArrayBuffers, so this wrapper
 * automatically converts or slices the input to a proper ArrayBuffer.
 *
 * @param {string} command
 * @param {ArrayBuffer | ArrayBufferView} data
 * @returns {Promise<ArrayBuffer>}
 */
export function invokeBinary(command, data) {
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
        return Promise.reject(
            new TypeError(
                `invokeBinary: expected ArrayBuffer or ArrayBufferView, got ${
                    data === null ? 'null' : typeof data
                }`
            )
        );
    }

    return window.core.invokeBinary(command, buffer);
}
