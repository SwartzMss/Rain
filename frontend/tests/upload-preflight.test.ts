// @vitest-environment node
import { File } from 'node:buffer';
import { describe, expect, it } from 'vitest';
import type { UploadLimits } from '../src/api/types';
import { preflightUpload } from '../src/features/files/uploadPreflight';

const limits: UploadLimits = {
  max_upload_bytes: 16 * 1024 ** 3, max_content_bytes: 8 * 1024 ** 3,
  used_content_bytes: 0, remaining_content_bytes: 8 * 1024 ** 3,
  max_archive_entries: 10000, max_compression_ratio: 1000
};

// Metadata-only fixtures deliberately have no payload: preflight must never inflate it.
function zip(entries: { name: string; size: number; compressed?: number }[], zip64 = false) {
  const records = entries.map((entry) => {
    const name = Buffer.from(entry.name);
    const record = Buffer.alloc(46 + name.length + (zip64 ? 20 : 0));
    record.writeUInt32LE(0x02014b50, 0);
    record.writeUInt32LE(zip64 ? 0xffffffff : entry.compressed ?? entry.size, 20);
    record.writeUInt32LE(zip64 ? 0xffffffff : entry.size, 24);
    record.writeUInt16LE(name.length, 28);
    record.writeUInt16LE(zip64 ? 20 : 0, 30);
    name.copy(record, 46);
    if (zip64) {
      const extra = 46 + name.length;
      record.writeUInt16LE(1, extra);
      record.writeUInt16LE(16, extra + 2);
      record.writeBigUInt64LE(BigInt(entry.size), extra + 4);
      record.writeBigUInt64LE(BigInt(entry.compressed ?? entry.size), extra + 12);
    }
    return record;
  });
  const directory = Buffer.concat(records);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(zip64 ? 0xffff : entries.length, 8);
  end.writeUInt16LE(zip64 ? 0xffff : entries.length, 10);
  end.writeUInt32LE(zip64 ? 0xffffffff : directory.length, 12);
  end.writeUInt32LE(zip64 ? 0xffffffff : 0, 16);
  const extraEnd = Buffer.alloc(zip64 ? 76 : 0);
  if (zip64) {
    extraEnd.writeUInt32LE(0x06064b50, 0);
    extraEnd.writeBigUInt64LE(44n, 4);
    extraEnd.writeBigUInt64LE(BigInt(entries.length), 24);
    extraEnd.writeBigUInt64LE(BigInt(entries.length), 32);
    extraEnd.writeBigUInt64LE(BigInt(directory.length), 40);
    extraEnd.writeUInt32LE(0x07064b50, 56);
    extraEnd.writeBigUInt64LE(BigInt(directory.length), 64);
    extraEnd.writeUInt32LE(1, 72);
  }
  return new File([directory, extraEnd, end], 'logs.zip') as unknown as globalThis.File;
}

describe('upload preflight', () => {
  it('checks the sum of plain files against remaining capacity, allowing the exact boundary', async () => {
    const files = [new File(['1234'], 'a.log'), new File(['56'], 'b.log')] as unknown as globalThis.File[];
    await expect(preflightUpload(files, { ...limits, remaining_content_bytes: 6 })).resolves.toBeNull();
    await expect(preflightUpload(files, { ...limits, remaining_content_bytes: 5 })).rejects.toThrow('Issue 剩余 5 B');
  });

  it('rejects raw upload limits before reading any file', async () => {
    const file = { name: 'large.zip', size: limits.max_upload_bytes + 1 } as globalThis.File;
    await expect(preflightUpload([file], limits)).rejects.toThrow('超过上传上限');
  });

  it('reads ZIP metadata and rejects combined extracted size', async () => {
    const file = zip([{ name: 'a.log', size: 80 }, { name: 'b.log', size: 80 }]);
    await expect(preflightUpload([file], { ...limits, max_content_bytes: 100 })).rejects.toThrow('超过解压上限');
  });

  it('checks ZIP content against capacity already used by an Issue', async () => {
    const file = zip([{ name: 'a.log', size: 80 }]);
    await expect(preflightUpload([file], { ...limits, remaining_content_bytes: 70 })).rejects.toThrow('Issue 剩余');
  });

  it('supports ZIP64 sizes over 4 GiB without reading the payload', async () => {
    const file = zip([{ name: 'large.log', size: 5 * 1024 ** 3 }], true);
    await expect(preflightUpload([file], limits)).resolves.toBeNull();
    await expect(preflightUpload([file], { ...limits, max_content_bytes: 4 * 1024 ** 3 })).rejects.toThrow('超过单文件上限');
  });

  it('sums extraction budgets across ZIPs', async () => {
    const file = zip([{ name: 'a.log', size: 60 }]);
    await expect(preflightUpload([file, file], { ...limits, max_content_bytes: 100 })).rejects.toThrow('超过解压上限');
  });

  it('checks compression ratio and cumulative entry limits', async () => {
    await expect(preflightUpload([zip([{ name: 'a.log', size: 1001, compressed: 1 }])], limits)).rejects.toThrow('压缩比');
    const file = zip([{ name: 'a.log', size: 1 }]);
    await expect(preflightUpload([file, file], { ...limits, max_archive_entries: 1 })).rejects.toThrow('条目总数');
  });

  it('does not count nested archive containers as final Issue content', async () => {
    const file = zip([{ name: 'nested.zip', size: 100 }, { name: 'a.log', size: 1 }]);
    await expect(preflightUpload([file], { ...limits, remaining_content_bytes: 1 })).resolves.toContain('无法提前计算');
  });

  it('warns for GZIP and unreadable ZIP metadata instead of treating the compressed size as content', async () => {
    for (const name of ['a.gz', 'a.tar.gz', 'a.tgz', 'a.zip']) {
      const file = new File(['unknown'], name) as unknown as globalThis.File;
      await expect(preflightUpload([file], { ...limits, remaining_content_bytes: 1 })).resolves.toContain('无法提前计算');
    }
  });
});
