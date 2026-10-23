import { describe, expect, it } from 'vitest';

import type { ApplicationDate, ApplicationUninstallCandidate } from '@/lib/models/applications/application';

function date(days: number): ApplicationDate {
  return {
    timestampMs: days * 86_400_000,
    source: 'platformReported',
    precision: 'day',
    capturedAtMs: 1_800_000_000_000,
    estimated: false,
    partial: false,
  };
}

import {
  applicationCatalogFilters,
  applicationMatchesCatalogFilter,
  applicationStatusKey,
  applicationCanStartUninstall,
  applicationSupportsUninstall,
  filterAndSortApplications,
  nextApplicationCatalogSort,
} from './application-uninstall-catalog';

function candidate(
  name: string,
  estimatedBytes: number,
  lastUsedAtMs: number | null,
  capability: ApplicationUninstallCandidate['capability'] = 'ready'
): ApplicationUninstallCandidate {
  return {
    applicationId: `application-${name}`,
    primaryIdentifier: `com.example.${name}`,
    systemKind: 'unclassified',
    sourceIdentities: [{ source: 'macosBundle', identifier: `com.example.${name}` }],
    name,
    version: null,
    publisher: 'Example',
    estimatedBytes,
    dates: { lastUsed: lastUsedAtMs === null ? null : date(lastUsedAtMs), installed: null, readStatus: 'complete' },
    platform: 'macosBundle',
    installerKind: null,
    executionMode: null,
    capability,
    recordState: 'installed',
    uninstallDiagnostic: null,
    applicationPath: null,
    possibleRelatedPaths: [],
    iconPath: null,
    runningProcesses: [],
    totalBytes: estimatedBytes,
    defaultSelectedBytes: estimatedBytes,
    associatedDataComplete: true,
    components: [],
  };
}

describe('application uninstall catalog', () => {
  const applications: ApplicationUninstallCandidate[] = [
    candidate('Small', 10, 100),
    candidate('Large', 30, null),
    candidate('Medium', 20, 300, 'applicationRunning'),
    {
      ...candidate('Elevated', 25, 250, 'requiresElevation'),
      platform: 'windowsRegistry' as const,
      dates: { lastUsed: date(250), installed: date(250), readStatus: 'complete' },
    },
    candidate('Unknown', 0, 200),
  ];

  it('sorts by size and uses the name as a stable tie breaker', () => {
    expect(filterAndSortApplications(applications, '', 'all', 'sizeDescending').map(item => item.name)).toEqual([
      'Large',
      'Elevated',
      'Medium',
      'Small',
      'Unknown',
    ]);
    expect(filterAndSortApplications(applications, '', 'all', 'sizeAscending').map(item => item.name)).toEqual([
      'Small',
      'Medium',
      'Elevated',
      'Large',
      'Unknown',
    ]);
  });

  it('keeps unavailable dates last in either direction', () => {
    expect(filterAndSortApplications(applications, '', 'all', 'lastUsedDescending').map(item => item.name)).toEqual([
      'Medium',
      'Elevated',
      'Unknown',
      'Small',
      'Large',
    ]);
    expect(filterAndSortApplications(applications, '', 'all', 'lastUsedAscending').map(item => item.name)).toEqual([
      'Small',
      'Unknown',
      'Elevated',
      'Medium',
      'Large',
    ]);
  });

  it('sorts Windows applications by their install or update date', () => {
    const windowsApplications = [
      {
        ...candidate('Older', 10, 900),
        platform: 'windowsRegistry' as const,
        dates: { lastUsed: date(900), installed: date(100), readStatus: 'complete' as const },
      },
      {
        ...candidate('Newer', 20, 100),
        platform: 'windowsRegistry' as const,
        dates: { lastUsed: date(100), installed: date(300), readStatus: 'complete' as const },
      },
      { ...candidate('Unknown date', 30, 500), platform: 'windowsRegistry' as const },
    ];

    expect(
      filterAndSortApplications(windowsApplications, '', 'all', 'installedDescending').map(item => item.name)
    ).toEqual(['Newer', 'Older', 'Unknown date']);
  });

  it('uses independent installation dates and alphabetical ties for the displayed local day', () => {
    const early = candidate('A early', 1, null);
    early.dates = {
      lastUsed: { ...date(1), precision: 'timestamp', timestampMs: Date.UTC(2024, 2, 5, 23) },
      installed: date(500),
      readStatus: 'complete',
    };
    const late = candidate('Z late', 1, null);
    late.dates = {
      lastUsed: { ...date(1), precision: 'timestamp', timestampMs: Date.UTC(2024, 2, 6, 1) },
      installed: date(100),
      readStatus: 'complete',
    };
    expect(
      filterAndSortApplications([early, late], '', 'all', 'lastUsedDescending', 'Asia/Shanghai').map(item => item.name)
    ).toEqual(['A early', 'Z late']);
    expect(filterAndSortApplications([early, late], '', 'all', 'installedDescending').map(item => item.name)).toEqual([
      'A early',
      'Z late',
    ]);
  });

  it('does not shift calendar-only evidence into the previous day west of UTC', () => {
    const day = candidate('Z day', 1, null);
    day.dates.lastUsed = { ...date(1), timestampMs: Date.UTC(2026, 9, 23) };
    const exact = candidate('A exact', 1, null);
    exact.dates.lastUsed = { ...date(1), precision: 'timestamp', timestampMs: Date.UTC(2026, 9, 23, 6) };
    expect(
      filterAndSortApplications([exact, day], '', 'all', 'lastUsedDescending', 'America/Los_Angeles').map(
        item => item.name
      )
    ).toEqual(['Z day', 'A exact']);
  });

  it('keeps elevated uninstallers actionable on both platforms', () => {
    const elevated = applications.find(item => item.name === 'Elevated');
    expect(elevated && applicationSupportsUninstall(elevated)).toBe(true);
    expect(
      elevated &&
        applicationSupportsUninstall({
          ...elevated,
          platform: 'macosBundle',
        })
    ).toBe(true);
    expect(filterAndSortApplications(applications, '', 'ready', 'nameAscending').map(item => item.name)).toEqual([
      'Elevated',
      'Large',
      'Small',
      'Unknown',
    ]);
    expect(filterAndSortApplications(applications, '', 'all', 'statusAscending').map(item => item.name)).toEqual([
      'Large',
      'Small',
      'Unknown',
      'Elevated',
      'Medium',
    ]);
  });

  it('allows a running application to enter the close-before-uninstall flow', () => {
    const running = applications.find(item => item.name === 'Medium');
    expect(running && applicationSupportsUninstall(running)).toBe(false);
    expect(running && applicationCanStartUninstall(running)).toBe(true);
  });

  it('uses platform-specific filters without changing the shared capability model', () => {
    expect(applicationCatalogFilters(true)).toEqual(['all', 'ready', 'running', 'unavailable']);
    expect(applicationCatalogFilters(false)).toEqual(['all', 'ready', 'requiresElevation', 'running', 'unavailable']);

    const windowsElevated = applications.find(item => item.name === 'Elevated');
    const macosElevated = candidate('Administrator owned', 35, null, 'requiresElevation');
    expect(windowsElevated && applicationMatchesCatalogFilter(windowsElevated, 'ready')).toBe(true);
    expect(windowsElevated && applicationMatchesCatalogFilter(windowsElevated, 'requiresElevation')).toBe(false);
    expect(applicationMatchesCatalogFilter(macosElevated, 'ready')).toBe(true);
    expect(applicationMatchesCatalogFilter(macosElevated, 'requiresElevation')).toBe(true);
    expect(applicationSupportsUninstall(macosElevated)).toBe(true);
  });

  it('combines capability filtering and text search', () => {
    expect(
      filterAndSortApplications(applications, 'medium', 'running', 'nameAscending').map(item => item.name)
    ).toEqual(['Medium']);
    expect(filterAndSortApplications(applications, 'medium', 'ready', 'nameAscending')).toEqual([]);
  });

  it.each(['c:', 'C:\\', ' c:/program files/ ', 'PROGRAM FILES\\EDITOR', 'editor.exe'])(
    'matches Windows application paths for query %s',
    query => {
      const installed = {
        ...candidate('Desktop app', 10, null),
        platform: 'windowsRegistry' as const,
        applicationPath: '\\\\?\\C:\\Program Files\\Editor\\editor.exe',
      };
      const otherDrive = { ...installed, name: 'Other app', applicationPath: 'D:\\Tools\\Viewer' };
      const unknown = { ...installed, name: 'Unknown location', applicationPath: null };

      expect(filterAndSortApplications([installed, otherDrive, unknown], query, 'all', 'nameAscending')).toEqual([
        installed,
      ]);
    }
  );

  it('combines drive searches with capability filters without matching related data paths', () => {
    const installed = {
      ...candidate('Desktop app', 10, null, 'applicationRunning'),
      platform: 'windowsRegistry' as const,
      applicationPath: 'D:/Tools/Editor',
      possibleRelatedPaths: ['C:\\Users\\fixture\\AppData\\Local\\Editor'],
    };

    expect(filterAndSortApplications([installed], 'd:\\', 'running', 'nameAscending')).toEqual([installed]);
    expect(filterAndSortApplications([installed], 'D:', 'ready', 'nameAscending')).toEqual([]);
    expect(filterAndSortApplications([installed], 'c:', 'all', 'nameAscending')).toEqual([]);
  });

  it('matches macOS paths and retains name, publisher, and identifier searches', () => {
    const installed = { ...candidate('Editor', 10, null), applicationPath: '/Applications/Editor.app' };
    const unknown = { ...candidate('Viewer', 20, null), publisher: 'Independent Vendor' };

    expect(filterAndSortApplications([installed, unknown], '/APPLICATIONS/', 'all', 'nameAscending')).toEqual([
      installed,
    ]);
    for (const query of ['viewer', 'independent vendor', 'com.example.Viewer']) {
      expect(filterAndSortApplications([installed, unknown], query, 'all', 'nameAscending')).toEqual([unknown]);
    }
  });

  it('keeps non-actionable entries in an explicit unavailable filter', () => {
    const unavailable = [
      ...applications,
      candidate('Administrator owned', 35, null, 'requiresElevation'),
      candidate('Protected', 40, null, 'protectedApplication'),
      candidate('View only', 50, null, 'viewOnly'),
    ];

    expect(
      filterAndSortApplications(unavailable, '', 'requiresElevation', 'nameAscending').map(item => item.name)
    ).toEqual(['Administrator owned']);
    expect(filterAndSortApplications(unavailable, '', 'unavailable', 'nameAscending').map(item => item.name)).toEqual([
      'Protected',
      'View only',
    ]);
  });

  it('labels an orphaned Windows registration separately from a generic unavailable entry', () => {
    const unavailable = {
      ...candidate('Removed', 0, null, 'viewOnly'),
      platform: 'windowsRegistry' as const,
    };

    expect(applicationStatusKey(unavailable)).toBe('viewOnly');
    expect(
      applicationStatusKey({
        ...unavailable,
        recordState: 'orphanedRegistration',
        uninstallDiagnostic: null,
        possibleRelatedPaths: ['C:\\Users\\fixture\\AppData\\Local\\com.example.removed'],
      })
    ).toBe('orphanedRegistration');
  });

  it('shows elevated Windows uninstallers as ready while preserving the macOS permission status', () => {
    const elevated = candidate('Elevated', 25, null, 'requiresElevation');
    expect(applicationStatusKey(elevated)).toBe('requiresElevation');
    expect(applicationStatusKey({ ...elevated, platform: 'windowsRegistry' })).toBe('readyForReview');
  });

  it('uses table-header sorting defaults and toggles the active column', () => {
    expect(nextApplicationCatalogSort('sizeDescending', 'name')).toBe('nameAscending');
    expect(nextApplicationCatalogSort('nameAscending', 'name')).toBe('nameDescending');
    expect(nextApplicationCatalogSort('nameDescending', 'lastUsed')).toBe('lastUsedDescending');
    expect(nextApplicationCatalogSort('lastUsedDescending', 'lastUsed')).toBe('lastUsedAscending');
    expect(nextApplicationCatalogSort('lastUsedAscending', 'installed')).toBe('installedDescending');
    expect(nextApplicationCatalogSort('installedAscending', 'status')).toBe('statusDescending');
  });
});
