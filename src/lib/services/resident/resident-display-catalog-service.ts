import type { ResidentReading } from '@/lib/models/resident/resident';
import { ResidentService } from '@/lib/services/resident/resident-service';

/** Owns one settings view's subscription, including retries and late-unmount cleanup. */
export class ResidentDisplayCatalogService {
  private disposed = false;
  private unlisten: (() => void) | null = null;
  private revision = -1;
  private loading = false;
  constructor(
    private readonly onReading: (reading: ResidentReading) => void,
    private readonly onError: (failed: boolean) => void
  ) {}
  private accept = (value: ResidentReading) => {
    if (this.disposed || value.schemaVersion !== 35 || value.revision < this.revision) return;
    this.revision = value.revision;
    this.onReading(value);
  };
  async load(): Promise<void> {
    // Retry must restore the subscription before reading the cached catalogue.
    if (this.loading) return;
    this.loading = true;
    this.onError(false);
    try {
      if (!this.unlisten) {
        const dispose = await ResidentService.onReading(this.accept);
        if (this.disposed) {
          dispose();
          return;
        }
        this.unlisten = dispose;
      }
      this.accept(await ResidentService.catalogue());
    } catch {
      if (!this.disposed) this.onError(true);
    } finally {
      this.loading = false;
    }
  }
  dispose(): void {
    this.disposed = true;
    this.unlisten?.();
  }
}
