---@meta

---@class HwUtils
local utils = {}

---@param text string
---@param sep string
---@return string[]
function utils.split(text, sep) end

---@param parts string[]
---@param sep string
---@return string
function utils.join(parts, sep) end

---@param text string
---@return number?
function utils.parse_number(text) end

---@param tbl table
---@return any[]
function utils.table_keys(tbl) end

---@param text string
---@param prefix string
---@return boolean
function utils.starts_with(text, prefix) end

---@param text string
---@param suffix string
---@return boolean
function utils.ends_with(text, suffix) end

---@param bytes integer
---@return string
function utils.format_size(bytes) end

return utils
