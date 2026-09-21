//! Values shared between ingestion, matching and publishing.

use crate::geo;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Vendor {
    Zirix,
    Conecta,
    Maxtrack,
    Sonda,
    BrtRt,
    Unknown,
}

impl Vendor {
    pub fn as_str(self) -> &'static str {
        match self {
            Vendor::Zirix => "zirix",
            Vendor::Conecta => "conecta",
            Vendor::Maxtrack => "maxtrack",
            Vendor::Sonda => "sonda",
            Vendor::BrtRt => "brt-rt",
            Vendor::Unknown => "unknown",
        }
    }

    pub fn from_feed(s: &str) -> Vendor {
        match s.trim().to_ascii_lowercase().as_str() {
            "zirix" => Vendor::Zirix,
            "conecta" => Vendor::Conecta,
            "maxtrack" => Vendor::Maxtrack,
            "sonda" => Vendor::Sonda,
            _ => Vendor::Unknown,
        }
    }

    pub fn code(self) -> u8 {
        self as u8
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Ida,
    Volta,
    Circular,
}

impl Dir {
    pub fn as_str(self) -> &'static str {
        match self {
            Dir::Ida => "ida",
            Dir::Volta => "volta",
            Dir::Circular => "circular",
        }
    }

    /// The GTFS `direction_id` a shape must carry to be compatible.
    /// A circular service matches either, since such routes have one shape.
    pub fn direction_id(self) -> Option<u8> {
        match self {
            Dir::Ida => Some(0),
            Dir::Volta => Some(1),
            Dir::Circular => None,
        }
    }

    pub fn from_feed(s: &str) -> Option<Dir> {
        match s.trim().to_ascii_lowercase().as_str() {
            "i" | "ida" | "0" => Some(Dir::Ida),
            "v" | "volta" | "1" => Some(Dir::Volta),
            "c" | "circular" | "unico" | "único" => Some(Dir::Circular),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceState {
    /// The vendor named a line.
    Named,
    /// This vendor did not send a service. It says nothing about the bus: the
    /// Zirix feed leaves it blank on 18% of rows while Conecta always fills it,
    /// so the last known service must be kept rather than the vehicle dropped.
    Unknown,
    /// The operator is saying the bus is not carrying passengers.
    OutOfService,
}

pub fn classify_service(service: &str) -> ServiceState {
    let s = service.trim();
    if s.is_empty() {
        return ServiceState::Unknown;
    }
    if is_out_of_service(s) {
        ServiceState::OutOfService
    } else {
        ServiceState::Named
    }
}

/// Codes the operators use to say a bus is not carrying passengers.
fn is_out_of_service(s: &str) -> bool {
    // "0" and "000" alike.
    if s.bytes().all(|c| c == b'0') {
        return true;
    }
    let upper = s.to_ascii_uppercase();
    // Garage runs come as "1 GAR", "GAR 2" and the like.
    if upper
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| w == "GAR" || w.starts_with("GARAG"))
    {
        return true;
    }
    matches!(
        upper.as_str(),
        "GARAGEM"
            | "MANUTENCAO"
            | "MANUTENÇÃO"
            | "RESERVADO"
            | "ESPECIAL"
            | "APOIO"
            | "FORA DE OP"
            | "FORA DE OPERACAO"
            | "FORA DE OPERAÇÃO"
            | "RECOLHE"
            | "RECOLHIDO"
            | "TREINAMENTO"
            | "TREINO"
            | "VISTORIA"
            | "SEM LINHA"
    )
}

/// One accepted position report.
#[derive(Clone, Debug)]
pub struct Fix {
    pub vehicle: String,
    pub vendor: Vendor,
    /// Device clock, unix seconds. All physics keys on this.
    pub t: i64,
    /// When the vendor's server published it, used only to measure pipeline lag.
    pub t_server: Option<i64>,
    pub lat: f64,
    pub lon: f64,
    pub x: f32,
    pub y: f32,
    pub heading: Option<f32>,
    pub service: Option<String>,
    pub dir: Option<Dir>,
    pub shape_id: Option<String>,
    pub trip_id: Option<String>,
    pub brt: bool,
}

impl Fix {
    pub fn new(vehicle: String, vendor: Vendor, t: i64, lat: f64, lon: f64) -> Fix {
        let (x, y) = geo::project(lat, lon);
        Fix {
            vehicle,
            vendor,
            t,
            t_server: None,
            lat,
            lon,
            x,
            y,
            heading: None,
            service: None,
            dir: None,
            shape_id: None,
            trip_id: None,
            brt: false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// Seen, but no confirmed direction yet: drawn on the map, never predicted.
    Pending,
    /// Has not moved in ten minutes and never confirmed a direction: a yard.
    Parked,
    /// Inside a known garage area: not carrying passengers.
    Garage,
    InProgress,
    Layover,
    Stale,
    OffRoute,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Pending => "pending",
            Phase::Parked => "parked",
            Phase::Garage => "garage",
            Phase::InProgress => "live",
            Phase::Layover => "layover",
            Phase::Stale => "stale",
            Phase::OffRoute => "offroute",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_state_codes_are_not_services() {
        for s in [
            "GARAGEM", "garagem", "Manutencao", "RESERVADO", "FORA DE OP", "0", "000", "1 GAR", "gar-2",
            "TREINO", "Vistoria",
        ] {
            assert_eq!(classify_service(s), ServiceState::OutOfService, "{s:?}");
        }
        for s in ["474", "SN238", "LECD138", "2343", "10", "001", "2305 - A"] {
            assert_eq!(classify_service(s), ServiceState::Named, "{s:?}");
        }
        // A blank service is one vendor staying quiet, not a parked bus.
        for s in ["", " "] {
            assert_eq!(classify_service(s), ServiceState::Unknown, "{s:?}");
        }
    }

    #[test]
    fn direction_parsing_covers_both_feeds() {
        // Per-vendor endpoints use I/V/C, the aggregator spells them out.
        assert_eq!(Dir::from_feed("I"), Some(Dir::Ida));
        assert_eq!(Dir::from_feed("ida"), Some(Dir::Ida));
        assert_eq!(Dir::from_feed("V"), Some(Dir::Volta));
        assert_eq!(Dir::from_feed("volta"), Some(Dir::Volta));
        assert_eq!(Dir::from_feed("C"), Some(Dir::Circular));
        assert_eq!(Dir::from_feed("circular"), Some(Dir::Circular));
        assert_eq!(Dir::from_feed("unico"), Some(Dir::Circular));
        assert_eq!(Dir::from_feed(""), None);
        assert_eq!(Dir::Ida.direction_id(), Some(0));
        assert_eq!(Dir::Circular.direction_id(), None);
    }
}
