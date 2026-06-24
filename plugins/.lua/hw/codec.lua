---@meta

---@class HwCodec
local codec = {}

---@param bytes string
---@return string
function codec.to_hex(bytes) end

---@param hex string
---@return string
function codec.from_hex(hex) end

---@param text string
---@return integer
function codec.xor8(text) end

---@param bytes string
---@return integer
function codec.crc16_modbus(bytes) end

---@param line string
---@return string
function codec.trim_line(line) end

---@param text string
---@return string[]
function codec.split_lines(text) end

return codec
