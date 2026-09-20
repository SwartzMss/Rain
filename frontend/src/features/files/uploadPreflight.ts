import type { UploadLimits } from '../../api/types';

const archiveName = /\.(zip|gz|tgz)$/i;
const MAX_DIRECTORY_BYTES = 16 * 1024 * 1024;
const bytes = (value: number) => {
  const unit = value >= 1024 ** 3 ? 3 : value >= 1024 ** 2 ? 2 : value >= 1024 ? 1 : 0;
  return `${(value / 1024 ** unit).toFixed(unit ? 2 : 0)} ${['B', 'KiB', 'MiB', 'GiB'][unit]}`;
};

class UnknownZip extends Error {}
const unknown = () => { throw new UnknownZip(); };

async function read(file: Blob, start: number, length: number): Promise<DataView> {
  if (!Number.isSafeInteger(start) || start < 0 || start + length > file.size) unknown();
  return new DataView(await file.slice(start, start + length).arrayBuffer());
}

function uint64(data: DataView, offset: number): number {
  if (offset + 8 > data.byteLength) return unknown();
  const value = data.getUint32(offset, true) + data.getUint32(offset + 4, true) * 2 ** 32;
  if (!Number.isSafeInteger(value)) return unknown();
  return value;
}

// Read only the central directory, never inflate payloads in the browser.
// ZIP/ZIP64 layout: https://pkware.cachefly.net/webdocs/casestudies/APPNOTE.TXT
async function inspectZip(file: File, limits: UploadLimits) {
  const tailStart = Math.max(0, file.size - 65557);
  const tail = await read(file, tailStart, file.size - tailStart);
  let end = tail.byteLength - 22;
  for (; end >= 0; end--) {
    if (tail.getUint32(end, true) === 0x06054b50 &&
        end + 22 + tail.getUint16(end + 20, true) === tail.byteLength) break;
  }
  if (end < 0) return unknown();
  if (tail.getUint16(end + 4, true) || tail.getUint16(end + 6, true)) return unknown();
  let count = tail.getUint16(end + 10, true);
  let size = tail.getUint32(end + 12, true);
  let offset = tail.getUint32(end + 16, true);
  let directoryEnd = tailStart + end;
  if (count === 0xffff || size === 0xffffffff || offset === 0xffffffff) {
    const locator = await read(file, directoryEnd - 20, 20);
    if (locator.getUint32(0, true) !== 0x07064b50 || locator.getUint32(4, true) !== 0 ||
        locator.getUint32(16, true) !== 1) return unknown();
    directoryEnd = uint64(locator, 8);
    const zip64 = await read(file, directoryEnd, 56);
    if (zip64.getUint32(0, true) !== 0x06064b50 || zip64.getUint32(16, true) ||
        zip64.getUint32(20, true) || uint64(zip64, 24) !== uint64(zip64, 32)) return unknown();
    count = uint64(zip64, 32);
    size = uint64(zip64, 40);
    offset = uint64(zip64, 48);
  } else if (tail.getUint16(end + 8, true) !== count) return unknown();
  if (count > limits.max_archive_entries) {
    throw new Error(`${file.name} 的压缩包条目数超过 ${limits.max_archive_entries}，未开始上传`);
  }
  if (size > MAX_DIRECTORY_BYTES || offset + size > directoryEnd) return unknown();
  const directory = await read(file, offset, size);
  let position = 0;
  let extracted = 0;
  let content = 0;
  let incomplete = false;
  for (let index = 0; index < count; index++) {
    if (position + 46 > size || directory.getUint32(position, true) !== 0x02014b50) return unknown();
    const flags = directory.getUint16(position + 8, true);
    let compressed = directory.getUint32(position + 20, true);
    let uncompressed = directory.getUint32(position + 24, true);
    const nameLength = directory.getUint16(position + 28, true);
    const extraLength = directory.getUint16(position + 30, true);
    const commentLength = directory.getUint16(position + 32, true);
    const extraStart = position + 46 + nameLength;
    const next = extraStart + extraLength + commentLength;
    if (next > size || flags & 1) return unknown();
    if (uncompressed === 0xffffffff || compressed === 0xffffffff) {
      let found = false;
      for (let extra = extraStart; extra + 4 <= extraStart + extraLength;) {
        const id = directory.getUint16(extra, true);
        const length = directory.getUint16(extra + 2, true);
        const extraEnd = extra + 4 + length;
        if (extraEnd > extraStart + extraLength) return unknown();
        if (id === 1) {
          let cursor = extra + 4;
          if (uncompressed === 0xffffffff) {
            if (cursor + 8 > extraEnd) return unknown();
            uncompressed = uint64(directory, cursor);
            cursor += 8;
          }
          if (compressed === 0xffffffff) {
            if (cursor + 8 > extraEnd) return unknown();
            compressed = uint64(directory, cursor);
          }
          found = true;
          break;
        }
        extra = extraEnd;
      }
      if (!found) return unknown();
    }
    const name = new TextDecoder().decode(new Uint8Array(directory.buffer, position + 46, nameLength));
    if (uncompressed > limits.max_content_bytes) {
      throw new Error(`${file.name} 中的 ${name} 解压后为 ${bytes(uncompressed)}，超过单文件上限 ${bytes(limits.max_content_bytes)}，未开始上传`);
    }
    if (uncompressed && (!compressed || Math.floor(uncompressed / compressed) > limits.max_compression_ratio)) {
      throw new Error(`${file.name} 中的 ${name} 压缩比超过服务器限制，未开始上传`);
    }
    extracted += uncompressed;
    if (!name.endsWith('/') && !name.endsWith('\\')) {
      if (archiveName.test(name)) incomplete = true;
      else content += uncompressed;
    }
    position = next;
  }
  return { extracted, content, count, incomplete };
}

export async function preflightUpload(files: File[], limits: UploadLimits): Promise<string | null> {
  const uploadBytes = files.reduce((sum, file) => sum + file.size, 0);
  if (uploadBytes > limits.max_upload_bytes) {
    throw new Error(`本次上传 ${bytes(uploadBytes)}，超过上传上限 ${bytes(limits.max_upload_bytes)}，未开始上传`);
  }
  let content = 0;
  let extracted = 0;
  let entries = 0;
  let incomplete = false;
  for (const file of files) {
    if (/\.zip$/i.test(file.name)) {
      try {
        const zip = await inspectZip(file, limits);
        content += zip.content;
        extracted += zip.extracted;
        entries += zip.count;
        incomplete ||= zip.incomplete;
      } catch (error) {
        if (!(error instanceof UnknownZip)) throw error;
        incomplete = true;
      }
    } else if (archiveName.test(file.name)) {
      incomplete = true;
    } else {
      content += file.size;
    }
    if (extracted > limits.max_content_bytes) {
      throw new Error(`本次压缩包解压大小至少 ${bytes(extracted)}，超过解压上限 ${bytes(limits.max_content_bytes)}，未开始上传`);
    }
    if (entries > limits.max_archive_entries) {
      throw new Error(`本次压缩包条目总数超过 ${limits.max_archive_entries}，未开始上传`);
    }
    if (content > limits.remaining_content_bytes) {
      throw new Error(`本次内容至少 ${bytes(content)}，Issue 剩余 ${bytes(limits.remaining_content_bytes)}（总上限 ${bytes(limits.max_content_bytes)}），未开始上传。请减少文件或清理已有内容。`);
    }
  }
  return incomplete ? '部分压缩内容无法提前计算完整大小，上传后仍可能因解压超限而失败。' : null;
}
